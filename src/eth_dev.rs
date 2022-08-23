//! EthDev wrapping
use crate::{mbuf::Mbuf, mempool::Mempool, tx_buffer::TxBuffer, Error, Result};
use dpdk_sys::*;
use std::{
    ffi::CString,
    fmt::Debug,
    mem::MaybeUninit,
    ops::Range,
    ptr::{self, NonNull},
};

#[derive(Debug)]
#[allow(missing_copy_implementations)]
/// An Ethernet device.
pub struct EthDev {
    port_id: u16,
}

#[allow(unsafe_code)]
impl EthDev {
    /// Get the number of ports which are usable for the application.
    pub fn avail_count() -> u32 {
        // SAFETY: ffi
        unsafe { rte_eth_dev_count_avail() as u32 }
    }

    fn port_is_valid(port_id: u16) -> bool {
        // SAFETY: ffi
        unsafe { rte_eth_dev_is_valid_port(port_id) == 1 }
    }

    /// Create an instance of EthDev.
    pub fn bind(port_id: u16) -> Result<Self> {
        if !Self::port_is_valid(port_id) {
            return Err(Error::InvalidArg);
        }
        Ok(Self { port_id })
    }

    /// Get socket id.
    pub fn socket_id(&self) -> u32 {
        // SAFETY: ffi
        unsafe { rte_eth_dev_socket_id(self.port_id) as u32 }
    }

    /// Check that numbers of Rx and Tx descriptors satisfy descriptors limits from the Ethernet device information,
    /// otherwise adjust them to boundaries. Then set up queues.
    pub fn queue_setup(
        &mut self,
        rx_queue_id: Range<u16>,
        tx_queue_id: Range<u16>,
        mut n_rxd: u16,
        mut n_txd: u16,
        rx_mp: &Mempool,
    ) -> Result<()> {
        // XXX: rx config and tx config
        // SAFETY: ffi
        let errno =
            unsafe { rte_eth_dev_adjust_nb_rx_tx_desc(self.port_id, &mut n_rxd, &mut n_txd) };
        Error::from_ret(errno)?;
        let socket_id = unsafe { rte_eth_dev_socket_id(self.port_id) };
        if socket_id < 0 {
            return Err(Error::InvalidArg); // port_id is invalid
        }
        let socket_id = socket_id as u32;

        for queue_id in rx_queue_id {
            // SAFETY: ffi
            let errno = unsafe {
                rte_eth_rx_queue_setup(
                    self.port_id,
                    queue_id,
                    n_rxd,
                    socket_id,
                    ptr::null(),
                    rx_mp.as_ptr(),
                )
            };
            Error::from_ret(errno)?;
        }

        for queue_id in tx_queue_id {
            // SAFETY: ffi
            let errno = unsafe {
                rte_eth_tx_queue_setup(self.port_id, queue_id, n_txd, socket_id, ptr::null())
            };
            Error::from_ret(errno)?;
        }
        Ok(())
    }

    /// Start an Ethernet device.
    ///
    /// The device start step is the last one and consists of setting the configured offload features and in starting the
    /// transmit and the receive units of the device. Device RTE_ETH_DEV_NOLIVE_MAC_ADDR flag causes MAC address to be
    /// set before PMD port start callback function is invoked.
    ///
    /// On success, all basic functions exported by the Ethernet API (link status, receive/transmit, and so on) can
    /// be invoked.
    pub fn start(&mut self) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_start(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Stop an Ethernet device.
    ///
    /// The device can be restarted with a call to `rte_eth_dev_start()`.
    pub fn stop(&mut self) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_stop(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Send a burst of  output packets on a transmit queue of an Ethernet device.
    pub fn tx_burst(&mut self, queue_id: u16, packets: &[Mbuf]) -> u16 {
        // TODO async
        let mut ptrs = packets.iter().map(|mbuf| mbuf.as_ptr()).collect::<Vec<_>>();
        // SAFETY: ffi
        unsafe { rte_eth_tx_burst(self.port_id, queue_id, ptrs.as_mut_ptr(), ptrs.len() as _) }
    }

    /// Retrieve a burst of input packets from a receive queue of an Ethernet device. The retrieved
    /// packets are stored in rte_mbuf structures whose pointers are supplied in the rx_pkts array.
    pub fn rx_burst(&mut self, queue_id: u16, n: u16) -> Vec<Mbuf> {
        // TODO async
        let mut ptrs = vec![];
        for _ in 0..n {
            ptrs.push(ptr::null_mut());
        }
        let n = unsafe { rte_eth_rx_burst(self.port_id, queue_id, ptrs.as_mut_ptr(), n) };
        let mut mbufs = vec![];
        for i in 0..n {
            mbufs.push(Mbuf::new_with_ptr(ptrs[i as usize]).unwrap());
        }
        mbufs
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
    pub fn buffer(&mut self, buf: &mut TxBuffer, pkt: &Mbuf, queue_id: u16) -> u16 {
        // TODO async
        // SAFETY: ffi
        unsafe { rte_eth_tx_buffer(self.port_id, queue_id, buf.as_ptr(), pkt.as_ptr()) }
    }

    /// Send any packets queued up for transmission on a port and HW queue.
    ///
    /// This causes an explicit flush of packets previously buffered via the rte_eth_tx_buffer()
    /// function. It returns the number of packets successfully sent to the NIC, and calls the
    /// error callback for any unsent packets. Unless explicitly set up otherwise, the default
    /// callback simply frees the unsent packets back to the owning mempool.
    pub fn flush(&mut self, buf: &mut TxBuffer, queue_id: u16) -> u16 {
        // TODO async
        // SAFETY: ffi
        unsafe { rte_eth_tx_buffer_flush(self.port_id, queue_id, buf.as_ptr()) }
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
    pub fn mac_addr(&self) -> Result<MacAddr> {
        let mut ether_addr = MaybeUninit::<rte_ether_addr>::uninit();
        // SAFETY: ffi
        let errno = unsafe { rte_eth_macaddr_get(self.port_id, ether_addr.as_mut_ptr()) };
        Error::from_ret(errno)?;
        // SAFETY: rte_ether_addr is successfully initialized due to no error code.
        let ether_addr = unsafe { ether_addr.assume_init() };
        Ok(MacAddr { ether_addr })
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

    /// Configure an Ethernet device. This function must be invoked first before any other function in the Ethernet
    /// API. This function can also be re-invoked when a device is in the stopped state.
    pub fn set_dev_config(&mut self, n_rxq: u16, n_txq: u16, conf: &EthConf) -> Result<()> {
        // SAFETY: ffi
        let errno =
            unsafe { rte_eth_dev_configure(self.port_id, n_rxq, n_txq, conf.inner.as_ptr()) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Link up an Ethernet device. Set device link up will re-enable the device Rx/Tx functionality after it is
    /// previously set device linked down.
    pub fn set_link_up(&mut self) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_set_link_up(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Link down an Ethernet device. The device Rx/Tx functionality will be disabled if success, and it can be
    /// re-enabled with a call to rte_eth_dev_set_link_up().
    pub fn set_link_down(&mut self) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_dev_set_link_down(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Enable receipt in promiscuous mode for an Ethernet device.
    pub fn enable_promiscuous(&mut self) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_promiscuous_enable(self.port_id) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Disable receipt in promiscuous mode for an Ethernet device.
    pub fn disable_promiscuous(&mut self) -> Result<()> {
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
        let errno = unsafe { rte_eth_dev_close(self.port_id) };
        Error::parse_err(errno);
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

/// Ethernet address: A universally administered address is uniquely assigned to a device by its manufacturer.
#[allow(missing_copy_implementations)]
pub struct MacAddr {
    ether_addr: rte_ether_addr,
}

impl Debug for MacAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MacAddr")
            .field("ether_addr", &self.ether_addr.addr_bytes)
            .finish()
    }
}

/// Device info.
#[allow(missing_copy_implementations)]
pub struct DevInfo {
    info: rte_eth_dev_info,
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

/// Device configuration.
#[derive(Debug, Copy, Clone)]
pub struct EthConf {
    inner: NonNull<rte_eth_conf>,
}
