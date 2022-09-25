//! EthDev wrapping
use crate::{
    agent::{RxAgent, TxAgent},
    mbuf::Mbuf,
    mempool::Mempool,
    protocol::Packet,
    Error, Result,
};
use dpdk_sys::*;
use std::{fmt::Debug, mem::MaybeUninit, ptr, sync::Arc};
use tokio::sync::mpsc;

#[allow(missing_copy_implementations)]
/// An Ethernet device.
pub struct EthDev {
    port_id: u16,
    n_rxd: u16,
    n_txd: u16,
    socket_id: u32,
    tx_agent: Arc<TxAgent>,
    rx_agent: Arc<RxAgent>,
    dev_info: rte_eth_dev_info,
    eth_conf: rte_eth_conf,
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
    pub fn new(port_id: u16, n_rxq: u16, n_txq: u16) -> Result<Arc<Self>> {
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

        let rx_agent = RxAgent::start();
        let tx_agent = TxAgent::start();

        Ok(Arc::new(Self {
            port_id,
            n_rxd,
            n_txd,
            socket_id,
            tx_agent,
            rx_agent,
            dev_info,
            eth_conf,
        }))
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
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_set_ptypes(self.port_id, 0, ptr::null_mut(), 0) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Stop an Ethernet device.
    pub async fn stop(self: &Arc<Self>) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_stop(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
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
    queue_id: u16,
    #[allow(dead_code)]
    mp: Mempool,
    dev: Arc<EthDev>,
}

/// An Ethernet device tx queue.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct EthTxQueue {
    queue_id: u16,
    mp: Mempool,
    tx: mpsc::Sender<Mbuf>,
    dev: Arc<EthDev>,
}

#[allow(unsafe_code)]
impl EthRxQueue {
    /// init
    pub fn init(dev: &Arc<EthDev>, queue_id: u16) -> Result<Arc<Self>> {
        let nb_ports = EthDev::available_ports();
        let mp = Mbuf::create_mp(
            format!("rx_{}_{}", dev.port_id, queue_id).as_str(),
            (nb_ports * (dev.n_rxd as u32 + dev.n_txd as u32 + 32)).max(8192),
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
        dev.rx_agent.register(dev.port_id, queue_id);
        Ok(Arc::new(Self {
            queue_id,
            mp,
            dev: dev.clone(),
        }))
    }
}

impl Drop for EthRxQueue {
    fn drop(&mut self) {
        self.dev
            .rx_agent
            .unregister(self.dev.port_id, self.queue_id);
    }
}

#[allow(unsafe_code)]
impl EthTxQueue {
    /// init
    pub fn init(dev: &Arc<EthDev>, queue_id: u16) -> Result<Arc<Self>> {
        let mp = Mbuf::create_mp(
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
        let tx = dev.tx_agent.register(dev.port_id, queue_id);
        Ok(Arc::new(Self {
            queue_id,
            mp,
            tx,
            dev: dev.clone(),
        }))
    }

    /// Send one packet.
    pub async fn send(&self, pkt: impl Packet) -> Result<()> {
        let mbuf = pkt.into_mbuf(&self.mp)?;
        self.send_m(mbuf).await
    }

    /// Send one Mbuf.
    #[inline(always)]
    async fn send_m(&self, msg: Mbuf) -> Result<()> {
        self.tx.send(msg).await.unwrap();
        Ok(())
    }
}

impl Drop for EthTxQueue {
    fn drop(&mut self) {
        self.dev
            .tx_agent
            .unregister(self.dev.port_id, self.queue_id);
    }
}
