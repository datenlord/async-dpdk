//! EthDev wrapping
use crate::{
    agent::{RxAgent, TxAgent},
    mbuf::Mbuf,
    mempool::Mempool,
    packet::Packet,
    Error, Result,
};
use dpdk_sys::*;
use std::{fmt::Debug, mem::MaybeUninit, ptr, sync::Arc};
use tokio::sync::mpsc;

#[allow(missing_copy_implementations)]
/// An Ethernet device.
pub struct EthDev {
    port_id: u16,
    socket_id: u32,
    tx_agent: Arc<TxAgent>,
    rx_agent: Arc<RxAgent>,
    tx_queue: Vec<Arc<EthTxQueue>>,
    rx_queue: Vec<Arc<EthRxQueue>>,
    tx_chan: Vec<Option<mpsc::Sender<Mbuf>>>,
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

        // XXX now we use one TxAgent and one RxAgent for each EthDev.
        // Make the mapping more flexible.
        let rx_agent = RxAgent::start();
        let tx_agent = TxAgent::start();

        let nb_ports = EthDev::available_ports();
        let n_elem = (nb_ports * (n_rxd as u32 + n_txd as u32 + 32)).max(8192);

        let mut tx_queue = vec![];
        let mut rx_queue = vec![];

        for queue_id in 0..n_txq {
            tx_queue.push(EthTxQueue::init(
                port_id, queue_id, socket_id, n_txd, &dev_info, &eth_conf,
            )?);
        }
        for queue_id in 0..n_rxq {
            rx_queue.push(EthRxQueue::init(
                port_id, queue_id, socket_id, n_rxd, n_elem, &dev_info, &eth_conf,
            )?);
        }

        let tx_chan = (0..n_txq).map(|_| None).collect();

        Ok(Self {
            port_id,
            socket_id,
            tx_agent,
            rx_agent,
            tx_queue,
            rx_queue,
            tx_chan,
        })
    }

    /// Get socket id.
    pub fn socket_id(&self) -> u32 {
        self.socket_id
    }

    /// Init tx queue and rx queue.
    fn start_queue(&mut self) -> Result<()> {
        for (queue_id, chan) in self.tx_chan.iter_mut().enumerate() {
            *chan = Some(self.tx_agent.register(self.port_id, queue_id as _));
        }
        self.rx_queue
            .iter()
            .enumerate()
            .for_each(|(queue_id, _)| self.rx_agent.register(self.port_id, queue_id as _));
        Ok(())
    }

    /// Stop tx queue and rx queue.
    fn stop_queue(&self) -> Result<()> {
        self.tx_queue
            .iter()
            .enumerate()
            .for_each(|(queue_id, _)| self.tx_agent.unregister(self.port_id, queue_id as _));
        self.rx_queue
            .iter()
            .enumerate()
            .for_each(|(queue_id, _)| self.rx_agent.unregister(self.port_id, queue_id as _));
        Ok(())
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
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_set_ptypes(self.port_id, 0, ptr::null_mut(), 0) };
        Error::from_ret(errno)?;
        self.start_queue()?;
        Ok(())
    }

    /// Stop an Ethernet device.
    pub fn stop(&mut self) -> Result<()> {
        self.stop_queue()?;
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_stop(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    pub(crate) fn sender(&self, queue_id: u16) -> Option<TxSender> {
        let chan: mpsc::Sender<Mbuf> = self.tx_chan.get(queue_id as usize)?.clone()?;
        let tx_queue: Arc<EthTxQueue> = self.tx_queue.get(queue_id as usize)?.clone();
        Some(TxSender {
            chan,
            tx_queue: tx_queue.clone(),
        })
    }

    /// Get MAC address.
    pub fn mac_addr(&self) -> Result<rte_ether_addr> {
        let mut ether_addr = MaybeUninit::<rte_ether_addr>::uninit();
        // SAFETY: ffi
        let errno = unsafe { rte_eth_macaddr_get(self.port_id, ether_addr.as_mut_ptr()) };
        Error::from_ret(errno)?;
        // SAFETY: rte_ether_addr is successfully initialized due to no error code.
        Ok(unsafe { ether_addr.assume_init() })
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

impl Drop for EthDev {
    fn drop(&mut self) {
        self.rx_agent.stop();
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
            .field("socket_id", &self.socket_id)
            .finish()
    }
}

/// An Ethernet device rx queue.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
struct EthRxQueue {
    #[allow(dead_code)]
    queue_id: u16,
    _mp: Mempool,
}

/// An Ethernet device tx queue.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
struct EthTxQueue {
    #[allow(dead_code)]
    queue_id: u16,
    mp: Mempool,
}

#[derive(Debug)]
pub(crate) struct TxSender {
    chan: mpsc::Sender<Mbuf>,
    tx_queue: Arc<EthTxQueue>,
}

impl TxSender {
    pub(crate) async fn send(&self, pkt: Packet) -> Result<()> {
        let m = pkt.into_mbuf(&self.tx_queue.mp)?;
        let res = self.chan.send(m).await;
        res.map_err(|_| Error::BrokenPipe)
    }
}

#[allow(unsafe_code)]
impl EthRxQueue {
    /// init
    fn init(
        port_id: u16,
        queue_id: u16,
        socket_id: u32,
        n_rxd: u16,
        n_elem: u32,
        dev_info: &rte_eth_dev_info,
        eth_conf: &rte_eth_conf,
    ) -> Result<Arc<Self>> {
        let mp = Mbuf::create_mp(
            format!("rx_{}_{}", port_id, queue_id).as_str(),
            n_elem,
            0,
            socket_id,
        )?;
        let mut rx_conf = dev_info.default_rxconf;
        rx_conf.offloads = eth_conf.rxmode.offloads;
        // SAFETY: ffi
        let errno = unsafe {
            rte_eth_rx_queue_setup(port_id, queue_id, n_rxd, socket_id, &rx_conf, mp.as_ptr())
        };
        Error::from_ret(errno)?;
        Ok(Arc::new(Self { queue_id, _mp: mp }))
    }
}

#[allow(unsafe_code)]
impl EthTxQueue {
    /// init
    fn init(
        port_id: u16,
        queue_id: u16,
        socket_id: u32,
        n_txd: u16,
        dev_info: &rte_eth_dev_info,
        eth_conf: &rte_eth_conf,
    ) -> Result<Arc<Self>> {
        let mp = Mbuf::create_mp(
            format!("tx_{}_{}", port_id, queue_id).as_str(),
            1024,
            0,
            socket_id,
        )?;
        let mut tx_conf = dev_info.default_txconf;
        tx_conf.offloads = eth_conf.txmode.offloads;
        // SAFETY: ffi
        let errno =
            unsafe { rte_eth_tx_queue_setup(port_id, queue_id, n_txd, socket_id, &tx_conf) };
        Error::from_ret(errno)?;
        Ok(Arc::new(Self { queue_id, mp }))
    }
}
