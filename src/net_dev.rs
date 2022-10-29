//! Net device.

use dpdk_sys::{
    cstring, rte_eth_dev_info, rte_eth_dev_info_get, rte_ether_addr, rte_free, rte_malloc,
};
use lazy_static::lazy_static;
use std::{mem, net::IpAddr, sync::RwLock};

use crate::{
    eth_dev::{EthDev, TxSender},
    Error, Result,
};

lazy_static! {
    /// Holding all probed Inet Devices.
    static ref INET_DEVICE: RwLock<Vec<InetDevice>> = RwLock::new(Vec::default());
}

/// Device that can be bound to using an IP address.
#[derive(Debug)]
struct InetDevice {
    /// The IP address assigned for the device.
    ip: IpAddr,
    /// Occupied `EthDev`.
    ethdev: EthDev,
    /// The device is started or not.
    running: bool,
}

/// Probe all devices.
#[allow(unsafe_code)]
#[allow(clippy::similar_names)] // tx and rx are DPDK terms
pub(crate) fn device_probe(addrs: Vec<IpAddr>) -> Result<()> {
    #[allow(clippy::unwrap_used)]
    let mut inet_device = INET_DEVICE.write().unwrap();
    if !inet_device.is_empty() {
        return Err(Error::Already);
    }
    let ndev = EthDev::available_ports();
    if (ndev as usize) < addrs.len() {
        return Err(Error::InvalidArg);
    }
    let ndev = addrs.len().min(ndev as usize);
    for (i, addr) in addrs.into_iter().enumerate().take(ndev) {
        let port_id = i as u16;
        // SAFETY: ffi
        let dev_info = unsafe {
            let dev_info = rte_malloc(
                cstring!("rte_eth_dev_info"),
                mem::size_of::<rte_eth_dev_info>(),
                0,
            );
            let errno = rte_eth_dev_info_get(port_id, dev_info.cast());
            Error::from_ret(errno)?;
            &mut *(dev_info.cast::<rte_eth_dev_info>())
        };
        let n_rxq = dev_info.max_rx_queues;
        let n_txq = dev_info.max_tx_queues;
        let ethdev = EthDev::new(port_id, n_rxq, n_txq)?;
        inet_device.push(InetDevice {
            ip: addr,
            ethdev,
            running: false,
        });
        // SAFETY: ffi, `dev_info`'s validity is checked upon its allocation
        #[allow(trivial_casts)]
        unsafe {
            rte_free((dev_info as *mut rte_eth_dev_info).cast());
        }
    }
    Ok(())
}

/// Start all probed devices.
#[inline]
pub fn device_start() -> Result<()> {
    #[allow(clippy::unwrap_used)]
    let mut inet_device = INET_DEVICE.write().unwrap();
    let inet_iter = inet_device.iter_mut();
    for dev in inet_iter {
        dev.ethdev.start()?;
        dev.running = true;
    }
    Ok(())
}

/// Stop all probed devices.
#[inline]
pub fn device_stop() -> Result<()> {
    #[allow(clippy::unwrap_used)]
    let mut inet_device = INET_DEVICE.write().unwrap();
    let inet_iter = inet_device.iter_mut();
    for dev in inet_iter {
        dev.ethdev.stop()?;
        dev.running = false;
    }
    Ok(())
}

/// Close all probed device.
pub(crate) fn device_close() {
    #[allow(clippy::unwrap_used)]
    let mut inet_device = INET_DEVICE.write().unwrap();
    inet_device.clear();
}

/// Get a device from an IP address.
pub(crate) fn find_dev_by_ip(ip: IpAddr) -> Result<(TxSender, rte_ether_addr)> {
    #[allow(clippy::unwrap_used)]
    let inet_device = INET_DEVICE.read().unwrap();
    let inet_iter = inet_device.iter();
    for dev in inet_iter {
        #[allow(clippy::else_if_without_else)] // continue if not matched
        if dev.ip == ip {
            if !dev.running {
                eprintln!("Device is not running!");
                return Err(Error::NoDev);
            }
            let sender = dev.ethdev.sender(0).ok_or(Error::NoDev)?;
            let addr = dev.ethdev.mac_addr()?;
            return Ok((sender, addr));
        } else if ip.is_unspecified() || ip.is_loopback() {
            if !dev.running {
                continue;
            }
            let sender = dev.ethdev.sender(0).ok_or(Error::NoDev)?;
            let addr = dev.ethdev.mac_addr()?;
            return Ok((sender, addr));
        }
    }
    Err(Error::InvalidArg)
}
