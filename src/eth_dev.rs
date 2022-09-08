//! EthDev wrapping
use crate::{eal::Eal, mbuf::Mbuf, mempool::Mempool, protocol::Packet, Error, Result};
use dpdk_sys::*;
use std::{
    fmt::Debug,
    mem::{self, MaybeUninit},
    ptr,
    sync::{Arc, Mutex},
};
use tokio::{
    sync::{mpsc, oneshot, Notify},
    task::{self, JoinHandle},
};

#[allow(missing_copy_implementations)]
/// An Ethernet device.
pub struct EthDev {
    port_id: u16,
    n_rxd: u16,
    n_txd: u16,
    socket_id: u32,
    dev_info: rte_eth_dev_info,
    eth_conf: rte_eth_conf,
    inner: Arc<Mutex<EthDevInner>>,
    _ctx: Arc<Eal>,
}

// This struct is to access interior immutability of EthDev. If the state of
// dev is changed, lock is acquired.
struct EthDevInner {
    recver: Option<Vec<mpsc::Sender<Mbuf>>>,
    sender: Option<Vec<mpsc::Receiver<Mbuf>>>,
    rx_recv: Vec<Option<mpsc::Receiver<Mbuf>>>,
    tx_send: Vec<Option<mpsc::Sender<Mbuf>>>,
    tx_task: Option<JoinHandle<()>>,
    rx_stop: Option<oneshot::Sender<Arc<Notify>>>,
}

const RX_CHAN_SIZE: usize = 1024;
const TX_CHAN_SIZE: usize = 1024;
const MAX_PKT_BURST: usize = 32;

#[allow(unsafe_code)]
impl EthDev {
    /// Get the number of ports which are usable for the application.
    pub fn available_ports() -> u32 {
        // SAFETY: ffi
        unsafe { rte_eth_dev_count_avail() as u32 }
    }

    /// Create an instance of EthDev.
    ///
    /// During this process, it does some initialization to the device:
    ///  1. Confugure the number of tx / rx queues.
    ///  2. Adjust the number of tx / rx desc.
    pub fn new(ctx: &Arc<Eal>, port_id: u16, n_rxq: u16, n_txq: u16) -> Result<Arc<Self>> {
        let mut dev_info = MaybeUninit::<rte_eth_dev_info>::uninit();
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_info_get(port_id, dev_info.as_mut_ptr()) };
        Error::from_ret(errno)?;
        let dev_info = unsafe { dev_info.assume_init() };

        // SAFETY: ffi
        let eth_conf = MaybeUninit::<rte_eth_conf>::zeroed();
        let mut eth_conf = unsafe { eth_conf.assume_init() };
        if dev_info.tx_offload_capa & RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE != 0 {
            // Enable fast release of mbufs if supported by the hardware.
            eth_conf.txmode.offloads |= RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE;
        }
        let errno = unsafe { rte_eth_dev_configure(port_id, n_rxq, n_txq, &eth_conf) };
        Error::from_ret(errno)?;
        let mut n_rxd = 1024;
        let mut n_txd = 1024;
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_adjust_nb_rx_tx_desc(port_id, &mut n_rxd, &mut n_txd) };
        Error::from_ret(errno)?;
        let socket_id = unsafe { rte_eth_dev_socket_id(port_id) };
        if socket_id < 0 {
            return Err(Error::InvalidArg); // port_id is invalid
        }
        let socket_id = socket_id as u32;

        let mut recver = vec![];
        let mut sender = vec![];
        let mut rx_recv = vec![];
        let mut tx_send = vec![];

        // TODO: start a stopped device
        for _ in 0..n_rxq {
            let (tx, rx) = mpsc::channel(RX_CHAN_SIZE);
            recver.push(tx); // used in a spawned task
            rx_recv.push(Some(rx)); // used by EthRxQueue
        }
        for _ in 0..n_txq {
            let (tx, rx) = mpsc::channel(TX_CHAN_SIZE);
            sender.push(rx); // used in a spawned task
            tx_send.push(Some(tx)); // used by EthTxQueue
        }

        let inner = Arc::new(Mutex::new(EthDevInner {
            recver: Some(recver),
            sender: Some(sender),
            rx_recv,
            tx_send,
            tx_task: None,
            rx_stop: None,
        }));

        Ok(Arc::new(Self {
            port_id,
            n_rxd,
            n_txd,
            socket_id,
            dev_info,
            eth_conf,
            inner,
            _ctx: ctx.clone(),
        }))
    }

    fn register_send_queue(self: &Arc<Self>, queue_id: u16) -> Result<mpsc::Sender<Mbuf>> {
        let mut inner = self.inner.lock().unwrap();
        inner.tx_send[queue_id as usize]
            .take()
            .ok_or(Error::Already)
    }

    fn register_recv_queue(self: &Arc<Self>, queue_id: u16) -> Result<mpsc::Receiver<Mbuf>> {
        let mut inner = self.inner.lock().unwrap();
        inner.rx_recv[queue_id as usize]
            .take()
            .ok_or(Error::Already)
    }

    /// Get socket id.
    pub fn socket_id(&self) -> u32 {
        self.socket_id
    }

    /// Start an Ethernet device.
    ///
    /// The device start step is the last one and consists of setting the configured offload
    /// features and in starting the transmit and the receive units of the device. Device
    /// RTE_ETH_DEV_NOLIVE_MAC_ADDR flag causes MAC address to be set before PMD port start
    /// callback function is invoked.
    ///
    /// On success, all basic functions exported by the Ethernet API (link status, receive/
    /// transmit, and so on) can be invoked.
    pub fn start(self: &Arc<Self>) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_start(self.port_id) };
        Error::from_ret(errno)?;
        let (rx_stop, rx) = oneshot::channel();
        let _ = self.start_rx(rx);
        let tx_task = self.start_tx();
        let mut inner = self.inner.lock().unwrap();
        inner.tx_task = Some(tx_task);
        inner.rx_stop = Some(rx_stop);
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_set_ptypes(self.port_id, 0, ptr::null_mut(), 0) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Stop an Ethernet device.
    pub async fn stop(self: &Arc<Self>) -> Result<()> {
        let notify = Arc::new(Notify::new());
        {
            let mut inner = self.inner.lock().unwrap();
            let rx = inner.rx_stop.take().ok_or(Error::Already)?;
            rx.send(notify.clone()).map_err(|_| Error::Already)?;
            let tx = inner.tx_task.take().ok_or(Error::Already)?;
            tx.abort();
        }
        notify.notified().await;
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_stop(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    fn start_rx(self: &Arc<Self>, mut stop: oneshot::Receiver<Arc<Notify>>) -> JoinHandle<()> {
        let port_id = self.port_id;
        let recver = {
            let mut inner = self.inner.lock().unwrap();
            inner.recver.take().unwrap()
        };
        task::spawn_blocking(move || {
            'rx: loop {
                for (queue_id, recver) in recver.iter().enumerate() {
                    if let Ok(notify) = stop.try_recv() {
                        notify.notify_one();
                        break 'rx;
                    }
                    let mut ptrs = vec![ptr::null_mut(); MAX_PKT_BURST];
                    // SAFETY: ffi
                    let n = unsafe {
                        rte_eth_rx_burst(
                            port_id,
                            queue_id as _,
                            ptrs.as_mut_ptr(),
                            MAX_PKT_BURST as _,
                        )
                    };
                    for i in 0..n as usize {
                        // XXX: channel out of memory
                        let _ = recver.try_send(Mbuf::new_with_ptr(ptrs[i]).unwrap()).ok();
                    }
                }
            }
        })
    }

    fn start_tx(self: &Arc<Self>) -> JoinHandle<()> {
        let port_id = self.port_id;
        let mut sender = {
            let mut inner = self.inner.lock().unwrap();
            inner.sender.take().unwrap()
        };
        task::spawn(async move {
            loop {
                for (queue_id, sender) in sender.iter_mut().enumerate() {
                    // TODO we should do burst tx
                    let m = sender.recv().await.unwrap();
                    let mut ptrs = vec![m.as_ptr()];
                    mem::forget(m);
                    // SAFETY: ffi
                    let _n = unsafe {
                        rte_eth_tx_burst(port_id, queue_id as _, ptrs.as_mut_ptr(), ptrs.len() as _)
                    };
                }
            }
        })
    }

    /// Get MAC address.
    pub fn mac_addr(port_id: u16) -> Result<rte_ether_addr> {
        let mut ether_addr = MaybeUninit::<rte_ether_addr>::uninit();
        // SAFETY: ffi
        let errno = unsafe { rte_eth_macaddr_get(port_id, ether_addr.as_mut_ptr()) };
        Error::from_ret(errno)?;
        // SAFETY: rte_ether_addr is successfully initialized due to no error code.
        Ok(unsafe { ether_addr.assume_init() })
    }

    /// Enable receipt in promiscuous mode for an Ethernet device.
    pub fn enable_promiscuous(self: &Arc<Self>) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_promiscuous_enable(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Disable receipt in promiscuous mode for an Ethernet device.
    pub fn disable_promiscuous(self: &Arc<Self>) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_promiscuous_disable(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Return the value of promiscuous mode for an Ethernet device.
    pub fn is_promiscuous(self: &Arc<Self>) -> bool {
        // SAFETY: ffi
        unsafe { rte_eth_promiscuous_get(self.port_id) == 1 }
    }

    fn get_ctx(self: &Arc<Self>) -> Arc<Eal> {
        self._ctx.clone()
    }
}

impl Drop for EthDev {
    fn drop(&mut self) {
        // SAFETY: ffi
        #[allow(unsafe_code)]
        let errno = unsafe { rte_eth_dev_close(self.port_id) };
        if errno < 0 {
            Error::parse_err(errno);
        }
    }
}

// SAFETY: EthDev can be globally accessed.
#[allow(unsafe_code)]
unsafe impl Send for EthDev {}

// SAFETY: EthDev can be globally accessed.
#[allow(unsafe_code)]
unsafe impl Sync for EthDev {}

impl Debug for EthDev {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthDev")
            .field("port_id", &self.port_id)
            .field("n_rxd", &self.n_rxd)
            .field("n_txd", &self.n_txd)
            .field("socket_id", &self.socket_id)
            .finish()
    }
}

/// An Ethernet device rx queue.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct EthRxQueue {
    rx: Arc<tokio::sync::Mutex<mpsc::Receiver<Mbuf>>>, // XXX no lock !!!
    #[allow(dead_code)]
    mp: Mempool,
    _dev: Arc<EthDev>,
}

/// An Ethernet device tx queue.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct EthTxQueue {
    mp: Mempool,
    tx: mpsc::Sender<Mbuf>,
    _dev: Arc<EthDev>,
}

#[allow(unsafe_code)]
impl EthRxQueue {
    /// init
    pub fn init(dev: &Arc<EthDev>, queue_id: u16) -> Result<Arc<Self>> {
        let nb_ports = EthDev::available_ports();
        let mp = Mbuf::create_mp(
            &dev.get_ctx(),
            format!("rx_{}_{}", dev.port_id, queue_id).as_str(),
            (nb_ports * (dev.n_rxd as u32 + dev.n_txd as u32 + MAX_PKT_BURST as u32)).max(8192),
            0,
            dev.socket_id(),
        )?;
        let mut rx_conf = dev.dev_info.default_rxconf;
        rx_conf.offloads = dev.eth_conf.rxmode.offloads;
        // SAFETY: ffi
        let errno = unsafe {
            rte_eth_rx_queue_setup(
                dev.port_id,
                queue_id,
                dev.n_rxd,
                dev.socket_id,
                &rx_conf,
                mp.as_ptr(),
            )
        };
        Error::from_ret(errno)?;
        let rx = dev.register_recv_queue(queue_id)?;
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        Ok(Arc::new(Self {
            rx,
            mp,
            _dev: dev.clone(),
        }))
    }

    /// Receive a Packet.
    pub async fn recv<P: Packet>(self: &Arc<Self>) -> Result<P> {
        let m = self.recv_m().await?;
        Ok(P::from_mbuf(m))
    }

    /// Receive one Mbuf.
    #[inline(always)]
    pub async fn recv_m(self: &Arc<Self>) -> Result<Mbuf> {
        let mut rx = self.rx.lock().await;
        rx.recv().await.ok_or(Error::IoErr)
    }
}

#[allow(unsafe_code)]
impl EthTxQueue {
    /// init
    pub fn init(dev: &Arc<EthDev>, queue_id: u16) -> Result<Arc<Self>> {
        let mp = Mbuf::create_mp(
            &dev.get_ctx(),
            format!("tx_{}_{}", dev.port_id, queue_id).as_str(),
            1024,
            0,
            dev.socket_id(),
        )?;
        let mut tx_conf = dev.dev_info.default_txconf;
        tx_conf.offloads = dev.eth_conf.txmode.offloads;
        // SAFETY: ffi
        let errno = unsafe {
            rte_eth_tx_queue_setup(dev.port_id, queue_id, dev.n_txd, dev.socket_id, &tx_conf)
        };
        Error::from_ret(errno)?;
        // XXX: what to do with buffer?
        // We should implement buffer here in Rust
        let tx = dev.register_send_queue(queue_id)?;
        Ok(Arc::new(Self {
            mp,
            tx,
            _dev: dev.clone(),
        }))
    }

    /// Send one packet.
    pub async fn send(&self, pkt: impl Packet) -> Result<()> {
        let mbuf = pkt.into_mbuf(&self.mp)?;
        self.send_m(mbuf).await
    }

    /// Send one Mbuf.
    pub async fn send_m(&self, msg: Mbuf) -> Result<()> {
        self.tx.send(msg).await.unwrap();
        Ok(())
    }
}
