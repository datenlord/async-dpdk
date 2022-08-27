//! EthDev wrapping
use crate::{buffer::TxBuffer, mbuf::Mbuf, mempool::Mempool, Error, Result};
use dpdk_sys::*;
use std::{
    ffi::CString,
    fmt::Debug,
    mem::{self, MaybeUninit},
    ops::{Deref, DerefMut},
    // os::raw::c_void,
    ptr,
};
use tokio::{
    sync::{mpsc, oneshot},
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
    recver: Option<Vec<mpsc::Sender<Mbuf>>>,
    sender: Option<Vec<mpsc::Receiver<Mbuf>>>,
    rx_recv: Vec<Option<mpsc::Receiver<Mbuf>>>,
    tx_send: Vec<Option<mpsc::Sender<Mbuf>>>,
    tx_task: Option<JoinHandle<()>>,
    rx_stop: Option<oneshot::Sender<()>>,
}

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
    pub fn new(port_id: u16, n_rxq: u16, n_txq: u16) -> Result<Self> {
        let mut dev_info = MaybeUninit::<rte_eth_dev_info>::uninit();
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_info_get(port_id, dev_info.as_mut_ptr()) };
        Error::from_ret(errno)?;
        let dev_info = unsafe { dev_info.assume_init() };

        // SAFETY: ffi
        let eth_conf = unsafe {
            let eth_conf = rte_zmalloc(cstring!("rte_eth_conf"), mem::size_of::<rte_eth_conf>(), 0)
                as *mut rte_eth_conf;
            if dev_info.tx_offload_capa & RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE != 0 {
                (*eth_conf).txmode.offloads |= RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE;
            }
            eth_conf
        };
        let errno = unsafe { rte_eth_dev_configure(port_id, n_rxq, n_txq, eth_conf) };
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

        for _ in 0..n_rxq {
            let (tx, rx) = mpsc::channel(1024);
            recver.push(tx);
            rx_recv.push(Some(rx));
        }
        for _ in 0..n_txq {
            let (tx, rx) = mpsc::channel(1024);
            sender.push(rx);
            tx_send.push(Some(tx));
        }

        Ok(Self {
            port_id,
            n_rxd,
            n_txd,
            socket_id,
            dev_info,
            eth_conf: unsafe { *eth_conf },
            recver: Some(recver),
            sender: Some(sender),
            rx_recv,
            tx_send,
            tx_task: None,
            rx_stop: None,
        })
    }

    fn register_send_queue(&mut self, queue_id: u16) -> mpsc::Sender<Mbuf> {
        self.tx_send[queue_id as usize].take().unwrap()
    }

    fn register_recv_queue(&mut self, queue_id: u16) -> mpsc::Receiver<Mbuf> {
        self.rx_recv[queue_id as usize].take().unwrap()
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
    pub fn start(&mut self) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_start(self.port_id) };
        Error::from_ret(errno)?;
        let (rx_stop, rx) = oneshot::channel();
        let _ = self.start_rx(rx);
        let tx_task = self.start_tx();
        self.tx_task = Some(tx_task);
        self.rx_stop = Some(rx_stop);
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_set_ptypes(self.port_id, 0, ptr::null_mut(), 0) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Stop an Ethernet device.
    pub async fn stop(&mut self) -> Result<()> {
        let rx = self.rx_stop.take().ok_or(Error::Already)?;
        rx.send(()).unwrap();
        let tx = self.tx_task.take().ok_or(Error::Already)?;
        tx.abort();
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_stop(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    fn start_rx(&mut self, mut stop: oneshot::Receiver<()>) -> JoinHandle<()> {
        let port_id = self.port_id;
        let recver = self.recver.take().unwrap();
        task::spawn_blocking(move || {
            'rx: loop {
                for (queue_id, recver) in recver.iter().enumerate() {
                    if stop.try_recv().is_ok() {
                        break 'rx;
                    }
                    let mut ptrs = vec![];
                    for _ in 0..8 {
                        ptrs.push(ptr::null_mut());
                    }
                    // SAFETY: ffi
                    let n =
                        unsafe { rte_eth_rx_burst(port_id, queue_id as _, ptrs.as_mut_ptr(), 8) };
                    for i in 0..n as usize {
                        let _ = recver.try_send(Mbuf::new_with_ptr(ptrs[i]).unwrap()).ok();
                    }
                }
            }
        })
    }

    fn start_tx(&mut self) -> JoinHandle<()> {
        let port_id = self.port_id;
        let mut sender = self.sender.take().unwrap();
        task::spawn(async move {
            loop {
                for (queue_id, sender) in sender.iter_mut().enumerate() {
                    let mbuf = sender.recv().await.unwrap();
                    let mut ptrs = vec![mbuf.as_ptr()];
                    // SAFETY: ffi
                    let _n = unsafe {
                        rte_eth_tx_burst(port_id, queue_id as _, ptrs.as_mut_ptr(), ptrs.len() as _)
                    };
                }
            }
        })
    }

    /// Retrieve the contextual information of an Ethernet device.
    pub fn dev_info(&self) -> Result<DevInfo> {
        let mut info = MaybeUninit::<rte_eth_dev_info>::uninit();
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_info_get(self.port_id, info.as_mut_ptr()) };
        Error::from_ret(errno)?;
        // SAFETY: rte_eth_dev_info is successfully initialized due to no error code.
        let info = unsafe { info.assume_init() };
        Ok(DevInfo { info })
    }

    /// Get MAC address.
    pub fn mac_addr(port_id: u16) -> Result<EthAddr> {
        let mut ether_addr = MaybeUninit::<rte_ether_addr>::uninit();
        // SAFETY: ffi
        let errno = unsafe { rte_eth_macaddr_get(port_id, ether_addr.as_mut_ptr()) };
        Error::from_ret(errno)?;
        // SAFETY: rte_ether_addr is successfully initialized due to no error code.
        let addr = unsafe { ether_addr.assume_init() };
        Ok(EthAddr { addr })
    }

    /// Retrieve the link status (up/down), the duplex mode (half/full), the negotiation (auto/fixed), and if
    /// available, the speed (Mbps).
    pub fn link(&self) -> Result<EthLink> {
        let mut link = MaybeUninit::<rte_eth_link>::uninit();
        // SAFETY: ffi
        let errno = unsafe { rte_eth_link_get_nowait(self.port_id, link.as_mut_ptr()) };
        Error::from_ret(errno)?;
        // SAFETY: rte_eth_link is successfully initialized due to no error code.
        let link = unsafe { link.assume_init() };
        Ok(EthLink { link })
    }

    /// Link up an Ethernet device. Set device link up will re-enable the device Rx/Tx functionality after it is
    /// previously set device linked down.
    pub fn set_link_up(&self) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_set_link_up(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Link down an Ethernet device. The device Rx/Tx functionality will be disabled if success, and it can be
    /// re-enabled with a call to rte_eth_dev_set_link_up().
    pub fn set_link_down(&self) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_set_link_down(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Enable receipt in promiscuous mode for an Ethernet device.
    pub fn enable_promiscuous(&self) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_promiscuous_enable(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Disable receipt in promiscuous mode for an Ethernet device.
    pub fn disable_promiscuous(&self) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_promiscuous_disable(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Return the value of promiscuous mode for an Ethernet device.
    pub fn is_promiscuous(&self) -> bool {
        // SAFETY: ffi
        unsafe { rte_eth_promiscuous_get(self.port_id) == 1 }
    }
}

#[allow(unsafe_code)]
impl Drop for EthDev {
    fn drop(&mut self) {
        // SAFETY: ffi
        // #[allow(trivial_casts)]
        // unsafe { rte_free(&mut self.eth_conf as *mut _ as *mut c_void) };
        let errno = unsafe { rte_eth_dev_close(self.port_id) };
        Error::parse_err(errno);
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
    rx: mpsc::Receiver<Mbuf>,
    _mp: Mempool,
}

/// An Ethernet device tx queue.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct EthTxQueue {
    port_id: u16,
    queue_id: u16,
    buffer: TxBuffer,
    tx: mpsc::Sender<Mbuf>,
}

#[allow(unsafe_code)]
impl EthRxQueue {
    /// init
    pub fn init(dev: &mut EthDev, queue_id: u16, mp: Mempool) -> Result<Self> {
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
        let rx = dev.register_recv_queue(queue_id);
        Ok(Self { rx, _mp: mp })
    }

    /// Receive one Mbuf.
    pub async fn recv(&mut self) -> Result<Mbuf> {
        self.rx
            .recv()
            .await
            .map_or(Err(Error::IoErr), |mbuf| Ok(mbuf))
    }
}

#[allow(unsafe_code)]
impl EthTxQueue {
    /// init
    pub fn init(dev: &mut EthDev, queue_id: u16) -> Result<Self> {
        let mut tx_conf = dev.dev_info.default_txconf;
        tx_conf.offloads = dev.eth_conf.txmode.offloads;
        // SAFETY: ffi
        let errno = unsafe {
            rte_eth_tx_queue_setup(dev.port_id, queue_id, dev.n_txd, dev.socket_id, &tx_conf)
        };
        Error::from_ret(errno)?;
        let buffer = TxBuffer::new_socket(dev.socket_id() as _, 512)?;
        let tx = dev.register_send_queue(queue_id);
        Ok(Self {
            port_id: dev.port_id,
            queue_id,
            buffer,
            tx,
        })
    }

    /// Send one Mbuf.
    pub async fn send(&mut self, msg: Mbuf) -> Result<()> {
        self.tx.send(msg).await.unwrap();
        Ok(())
    }

    /// Buffer a single packet for future transmission on a port and queue.
    ///
    /// This function takes a single mbuf/packet and buffers it for later transmission on the
    /// particular port and queue specified. Once the buffer is full of packets, an attempt will
    /// be made to transmit all the buffered packets. In case of error, where not all packets
    /// can be transmitted, a callback is called with the unsent packets as a parameter. If no
    /// callback is explicitly set up, the unsent packets are just freed back to the owning
    /// mempool. The function returns the number of packets actually sent i.e. 0 if no buffer
    /// flush occurred, otherwise the number of packets successfully flushed.
    pub fn buffer(&mut self, pkt: &Mbuf) -> u16 {
        // TODO async
        // SAFETY: ffi
        unsafe {
            rte_eth_tx_buffer(
                self.port_id,
                self.queue_id,
                self.buffer.as_ptr(),
                pkt.as_ptr(),
            )
        }
    }

    /// Send any packets queued up for transmission on a port and HW queue.
    ///
    /// This causes an explicit flush of packets previously buffered via the rte_eth_tx_buffer()
    /// function. It returns the number of packets successfully sent to the NIC, and calls the
    /// error callback for any unsent packets. Unless explicitly set up otherwise, the default
    /// callback simply frees the unsent packets back to the owning mempool.
    pub fn flush_buffer(&mut self) -> u16 {
        // TODO async
        // SAFETY: ffi
        unsafe { rte_eth_tx_buffer_flush(self.port_id, self.queue_id, self.buffer.as_ptr()) }
    }
}

/// A structure used to retrieve link-level information of an Ethernet port.
#[allow(missing_copy_implementations)]
pub struct EthLink {
    link: rte_eth_link,
}

impl Debug for EthLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthLink")
            .field("link_speed", &self.link.link_speed)
            .field("link_duplex", &self.link.link_duplex())
            .field("link_autoneg", &self.link.link_autoneg())
            .field("link_status", &self.link.link_status())
            .finish()
    }
}

/// Ether header.
#[allow(missing_copy_implementations)]
#[allow(missing_debug_implementations)]
pub struct EtherHdr {
    hdr: rte_ether_hdr,
}

impl Deref for EtherHdr {
    type Target = rte_ether_hdr;

    fn deref(&self) -> &Self::Target {
        &self.hdr
    }
}

impl DerefMut for EtherHdr {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.hdr
    }
}

/// Ether address.
#[allow(missing_copy_implementations)]
pub struct EthAddr {
    addr: rte_ether_addr,
}

impl Deref for EthAddr {
    type Target = rte_ether_addr;

    fn deref(&self) -> &Self::Target {
        &self.addr
    }
}

impl DerefMut for EthAddr {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.addr
    }
}

impl Debug for EthAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthAddr")
            .field("addr", &self.addr.addr_bytes)
            .finish()
    }
}

/// Device info.
#[allow(missing_copy_implementations)]
pub struct DevInfo {
    info: rte_eth_dev_info,
}

impl Deref for DevInfo {
    type Target = rte_eth_dev_info;

    fn deref(&self) -> &Self::Target {
        &self.info
    }
}

impl DerefMut for DevInfo {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.info
    }
}

impl Debug for DevInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[allow(unsafe_code)]
        f.debug_struct("DevInfo")
            .field("driver_name", unsafe {
                &CString::from_raw(self.info.driver_name as *mut i8)
                    .into_string()
                    .unwrap()
            })
            .field("min_mtu", &self.info.min_mtu)
            .field("max_mtu", &self.info.max_mtu)
            .field("nb_tx_queues", &self.info.nb_tx_queues)
            .field("nb_rx_queues", &self.info.nb_rx_queues)
            .field("max_tx_queues", &self.info.max_tx_queues)
            .field("max_rx_queues", &self.info.max_rx_queues)
            .finish()
    }
}
