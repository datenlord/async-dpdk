//! An Ethernet device is corresponding to an UIO/VFIO device. DPDK provides APIs to configure
//! and poll those devices in user space. This module provide safe wrappings for devices and
//! queues. For more information, please refer to [`PMD document`].
//!
//! [`PMD document`]: https://doc.dpdk.org/guides/prog_guide/poll_mode_drv.html#poll-mode-driver

use crate::{
    agent::{RxAgent, TxAgent},
    mbuf::Mbuf,
    mempool::{Mempool, PktMempool},
    packet::Packet,
    Error, Result,
};
use dpdk_sys::{
    rte_eth_conf, rte_eth_dev_adjust_nb_rx_tx_desc, rte_eth_dev_close, rte_eth_dev_configure,
    rte_eth_dev_count_avail, rte_eth_dev_info, rte_eth_dev_info_get, rte_eth_dev_set_ptypes,
    rte_eth_dev_socket_id, rte_eth_dev_start, rte_eth_dev_stop, rte_eth_macaddr_get,
    rte_eth_rx_queue_setup, rte_eth_tx_queue_setup, rte_ether_addr,
    RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE,
};
use std::{fmt::Debug, mem::MaybeUninit, ptr, sync::Arc};
use tokio::sync::mpsc;

/// An Ethernet device.
///
/// It is identified with a `port_id`. Each `EthDev` has several tx queues and rx queues,
/// which are polled by agent threads.
///
/// ```text
///     new         Configure the device then initialize tx/rx queues.
///      |
///      v
///    start <--    Start agent threads and register queues on them.
///      |     |
///      v     |
///    stop-----    Stop sgent threads and unregister queues.
///      |          Stopped devices can be started again.
///      v
///    drop         Close the device.
/// ```
#[allow(missing_copy_implementations)]
pub(crate) struct EthDev {
    /// `port_id` identifying this `EthDev`.
    port_id: u16,
    /// `socket_id` that this `EthDev` is on.
    socket_id: i32,
    /// An agent tx thread if the device is started.
    tx_agent: Option<Arc<TxAgent>>,
    /// An agent rx thread if the device is started.
    rx_agent: Option<Arc<RxAgent>>,
    /// An agent tx thread if the device is started.
    tx_queue: Vec<Arc<EthTxQueue>>,
    /// `EthRxQueue` for each queue.
    rx_queue: Vec<Arc<EthRxQueue>>,
    /// `TxSender` to send `Mbuf`s to `tx_queue`.
    tx_chan: Vec<Option<mpsc::Sender<Mbuf>>>,
}

#[allow(unsafe_code)]
impl EthDev {
    /// Get the number of ports which are usable for the application.
    #[inline]
    #[must_use]
    pub(crate) fn available_ports() -> u32 {
        // SAFETY: ffi
        unsafe { u32::from(rte_eth_dev_count_avail()) }
    }

    /// Create an instance of `EthDev`.
    ///
    /// During this process, it does some initialization to the device:
    ///  1. Confugure the number of tx / rx queues.
    ///  2. Adjust the number of tx / rx desc.
    ///
    /// # Errors
    ///
    /// Possible reasons:
    ///  - `Error::NotSupported`: this device does not support getting info.
    ///  - `Error::NoDev`: invalid `port_id`.
    ///  - `Error::InvalidArg`: invalid `n_rxq` or `n_txq`.
    ///  - Failed to configure devices.
    ///  - Failed to setup `RxQueue` and `TxQueue`.
    #[inline]
    #[allow(clippy::similar_names)] // tx and rx are DPDK terms
    pub(crate) fn new(port_id: u16, n_rxq: u16, n_txq: u16) -> Result<Self> {
        let mut dev_info = MaybeUninit::<rte_eth_dev_info>::uninit();
        // SAFETY: the returned `dev_info` is to be verified with the check on errno
        let errno = unsafe { rte_eth_dev_info_get(port_id, dev_info.as_mut_ptr()) };
        Error::from_ret(errno)?;
        // SAFETY: `dev_info` init in `rte_eth_dev_info_get`
        let dev_info = unsafe { dev_info.assume_init() };

        let eth_conf = MaybeUninit::<rte_eth_conf>::zeroed();
        // SAFETY: `eth_conf` set to zero, which is valid
        let mut eth_conf = unsafe { eth_conf.assume_init() };
        if dev_info.tx_offload_capa & RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE != 0 {
            // Enable fast release of mbufs if supported by the hardware.
            eth_conf.txmode.offloads |= RTE_ETH_TX_OFFLOAD_MBUF_FAST_FREE;
        }
        // SAFETY: `eth_conf` is ok to be zerod, representing default configuration
        #[allow(clippy::shadow_unrelated)] // is related
        let errno = unsafe { rte_eth_dev_configure(port_id, n_rxq, n_txq, &eth_conf) };
        Error::from_ret(errno)?;
        log::trace!("Device {port_id} successfully configured");
        let mut n_rxd = 1024;
        let mut n_txd = 1024;
        // SAFETY: ffi
        #[allow(clippy::shadow_unrelated)] // is related
        let errno = unsafe { rte_eth_dev_adjust_nb_rx_tx_desc(port_id, &mut n_rxd, &mut n_txd) };
        Error::from_ret(errno)?;
        // SAFETY: returned `socket_id` to be checked later
        let socket_id = unsafe { rte_eth_dev_socket_id(port_id) };
        if socket_id < 0 {
            return Err(Error::InvalidArg); // port_id is invalid
        }

        let nb_ports = EthDev::available_ports();
        let n_elem = nb_ports
            .saturating_mul(
                u32::from(n_rxd)
                    .saturating_add(u32::from(n_txd))
                    .saturating_add(32),
            )
            .max(8192);

        let mut tx_queue = vec![];
        let mut rx_queue = vec![];

        for queue_id in 0..n_txq {
            tx_queue.push(EthTxQueue::init(
                port_id, queue_id, socket_id, n_txd, &dev_info, &eth_conf,
            )?);
            log::trace!("Device {port_id} successfully initialized tx_queue {queue_id}");
        }
        for queue_id in 0..n_rxq {
            rx_queue.push(EthRxQueue::init(
                port_id, queue_id, socket_id, n_rxd, n_elem, &dev_info, &eth_conf,
            )?);
            log::trace!("Device {port_id} successfully initialized rx_queue {queue_id}");
        }

        let tx_chan = (0..n_txq).map(|_| None).collect();

        Ok(Self {
            port_id,
            socket_id,
            tx_agent: None,
            rx_agent: None,
            tx_queue,
            rx_queue,
            tx_chan,
        })
    }

    /// Get port id.
    #[inline]
    #[must_use]
    pub(crate) fn port_id(&self) -> u16 {
        self.port_id
    }

    /// Start an Ethernet device.
    ///
    /// Register all `TxQueue`s and `RxQueue`s on agent threads and start polling. On success, all
    /// basic functions exported by the Ethernet API (link status, receive/transmit, and so on)
    /// can be invoked.
    ///
    /// # Errors
    ///
    /// Possible reasons:
    /// - `Error::TempUnavail`: temporary error, retry later.
    /// - Failed to create a `TxAgent`.
    /// - Failed to register queues on `TxAgent` and `RxAgent`.
    #[inline]
    pub(crate) fn start(&mut self) -> Result<()> {
        // XXX now we use one TxAgent and one RxAgent for each EthDev.
        // Make the mapping more flexible.
        let rx_agent = RxAgent::start(self.socket_id);
        let tx_agent = TxAgent::start();

        // SAFETY: `port_id` validity verified
        let errno = unsafe { rte_eth_dev_start(self.port_id) };
        Error::from_ret(errno)?;
        log::debug!("Device {} successfully started", self.port_id);
        // SAFETY: `ptypes` is ok to be NULL
        #[allow(clippy::shadow_unrelated)] // is related
        let errno = unsafe { rte_eth_dev_set_ptypes(self.port_id, 0, ptr::null_mut(), 0) };
        Error::from_ret(errno)?;

        // Start tx agent
        #[allow(clippy::cast_possible_truncation)] // self.tx_queue.len() checked
        for (queue_id, chan) in self.tx_chan.iter_mut().enumerate() {
            *chan = Some(tx_agent.register(self.port_id, queue_id as _)?);
        }

        // Start rx agent
        #[allow(clippy::cast_possible_truncation)] // self.rx_queue.len() checked
        for (queue_id, _) in self.rx_queue.iter().enumerate() {
            rx_agent.register(self.port_id, queue_id as _)?;
        }

        self.rx_agent = Some(rx_agent);
        self.tx_agent = Some(tx_agent);

        Ok(())
    }

    /// Stop an Ethernet device.
    ///
    /// # Errors
    ///
    /// Possible reasons:
    ///  - Agent threads are terminated.
    ///  - `Error::Busy`: unable to stop the device.
    #[inline]
    pub(crate) fn stop(&mut self) -> Result<()> {
        let rx_agent = self.rx_agent.take().ok_or(Error::BrokenPipe)?;
        let tx_agent = self.tx_agent.take().ok_or(Error::BrokenPipe)?;

        #[allow(clippy::cast_possible_truncation)] // self.tx_queue.len() checked
        for (queue_id, _) in self.tx_queue.iter().enumerate() {
            tx_agent.unregister(self.port_id, queue_id as _)?;
        }

        #[allow(clippy::cast_possible_truncation)] // self.rx_queue.len() checked
        for (queue_id, _) in self.rx_queue.iter().enumerate() {
            rx_agent.unregister(self.port_id, queue_id as _)?;
        }

        rx_agent.stop();
        // SAFETY: `port_id` validity verified
        let errno = unsafe { rte_eth_dev_stop(self.port_id) };
        Error::from_ret(errno)?;
        log::debug!("Device {} successfully stopped", self.port_id);
        Ok(())
    }

    /// Get a `TxSender`.
    ///
    /// This function returns None if the `queue_id` is invalid or the queue is
    /// not registered yet.
    pub(crate) fn sender(&self, queue_id: u16) -> Option<TxSender> {
        let chan: mpsc::Sender<Mbuf> = self.tx_chan.get(queue_id as usize)?.clone()?;
        let tx_queue: Arc<EthTxQueue> = Arc::clone(self.tx_queue.get(queue_id as usize)?);
        Some(TxSender { chan, tx_queue })
    }

    /// Get MAC address.
    #[inline]
    pub(crate) fn mac_addr(&self) -> Result<rte_ether_addr> {
        let mut ether_addr = MaybeUninit::<rte_ether_addr>::uninit();
        // SAFETY: errno checked later
        let errno = unsafe { rte_eth_macaddr_get(self.port_id, ether_addr.as_mut_ptr()) };
        Error::from_ret(errno)?;
        // SAFETY: `rte_ether_addr` is successfully initialized due to no error code.
        Ok(unsafe { ether_addr.assume_init() })
    }
}

impl Drop for EthDev {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: `port_id` validity verified
        #[allow(unsafe_code)]
        let errno = unsafe { rte_eth_dev_close(self.port_id) };
        if errno < 0 {
            Error::parse_err(errno);
        }
    }
}

// SAFETY: no thread-local data involved.
#[allow(unsafe_code)]
unsafe impl Send for EthDev {}

// SAFETY: EthDev can be globally accessed.
#[allow(unsafe_code)]
unsafe impl Sync for EthDev {}

impl Debug for EthDev {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthDev")
            .field("port_id", &self.port_id)
            .field("socket_id", &self.socket_id)
            .finish()
    }
}

/// A wrapper for channel to send Mbuf from socket to `EthTxQueue`.
#[derive(Debug)]
pub(crate) struct TxSender {
    /// The sender held by socket.
    chan: mpsc::Sender<Mbuf>, // TODO add a oneshot sender
    /// The `EthTxQueue` that this request is sent to.
    tx_queue: Arc<EthTxQueue>,
}

impl TxSender {
    /// Send a request to `TxAgent`
    pub(crate) async fn send(&self, pkt: Packet) -> Result<()> {
        let m = pkt.into_mbuf(&self.tx_queue.mp)?;
        self.chan.send(m).await.map_err(Error::from)
    }
}

/// An Ethernet device rx queue.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
struct EthRxQueue {
    /// The `queue_id` refered to this `EthTxQueue`.
    #[allow(dead_code)]
    queue_id: u16,
    /// `Mempool` to allocate `Mbuf`s to hold the received frames.
    _mp: PktMempool,
}

/// An Ethernet device tx queue.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
struct EthTxQueue {
    /// The `queue_id` refered to this `EthTxQueue`.
    #[allow(dead_code)]
    queue_id: u16,
    /// `Mempool` to allocate `Mbuf`s to send.
    mp: PktMempool,
}

#[allow(unsafe_code)]
impl EthRxQueue {
    /// Init rx queue.
    ///
    /// Create a `Mempool` to hold received packets, then setup the queue.
    fn init(
        port_id: u16,
        queue_id: u16,
        socket_id: i32,
        n_rxd: u16,
        n_elem: u32,
        dev_info: &rte_eth_dev_info,
        eth_conf: &rte_eth_conf,
    ) -> Result<Arc<Self>> {
        let socket_id: u32 = socket_id.try_into().map_err(Error::from)?;
        let mp = PktMempool::create(format!("rx_{port_id}_{queue_id}").as_str(), n_elem)?;
        let mut rx_conf = dev_info.default_rxconf;
        rx_conf.offloads = eth_conf.rxmode.offloads;
        // SAFETY: `mp` checked in initialization
        let errno = unsafe {
            rte_eth_rx_queue_setup(port_id, queue_id, n_rxd, socket_id, &rx_conf, mp.as_ptr())
        };
        Error::from_ret(errno)?;
        Ok(Arc::new(Self { queue_id, _mp: mp }))
    }
}

#[allow(unsafe_code)]
impl EthTxQueue {
    /// Init tx queue.
    ///
    /// Create a `Mempool` to hold packets to be sent, then setup the queue.
    fn init(
        port_id: u16,
        queue_id: u16,
        socket_id: i32,
        n_txd: u16,
        dev_info: &rte_eth_dev_info,
        eth_conf: &rte_eth_conf,
    ) -> Result<Arc<Self>> {
        let socket_id: u32 = socket_id.try_into().map_err(Error::from)?;
        let mp = PktMempool::create(format!("tx_{port_id}_{queue_id}").as_str(), 1024)?;
        let mut tx_conf = dev_info.default_txconf;
        tx_conf.offloads = eth_conf.txmode.offloads;
        // SAFETY: ffi
        let errno =
            unsafe { rte_eth_tx_queue_setup(port_id, queue_id, n_txd, socket_id, &tx_conf) };
        Error::from_ret(errno)?;
        Ok(Arc::new(Self { queue_id, mp }))
    }
}

#[cfg(test)]
mod tests {
    use super::EthDev;
    use crate::test_utils;

    #[tokio::test]
    async fn test() {
        test_utils::dpdk_setup();
        let mut dev = EthDev::new(0, 1, 1).unwrap();
        dev.start().unwrap();
        dev.stop().unwrap();
        dev.start().unwrap();
        dev.stop().unwrap();
        // `dev` drop here
    }
}
