//! Net device.

use crate::{
    eth_dev::{EthDev, TxSender},
    Error, Result,
};
use dpdk_sys::{
    cstring, rte_eth_dev_info, rte_eth_dev_info_get, rte_ether_addr, rte_free, rte_malloc,
};
use lazy_static::lazy_static;
use log::{debug, error};
use std::{mem, net::IpAddr, sync::RwLock};

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
    let mut inet_device = INET_DEVICE.write().map_err(Error::from)?;
    if !inet_device.is_empty() {
        return Err(Error::Already);
    }
    let ndev = EthDev::available_ports();
    if (ndev as usize) < addrs.len() {
        return Err(Error::InvalidArg);
    }
    let ndev = addrs.len().min(ndev as usize);
    for (i, addr) in addrs.into_iter().enumerate().take(ndev) {
        #[allow(clippy::cast_possible_truncation)]
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
        debug!("Ethdev {port_id} probed, bound to {addr:?}");
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
    let mut inet_device = INET_DEVICE.write().map_err(Error::from)?;
    let inet_iter = inet_device.iter_mut();
    for dev in inet_iter {
        dev.ethdev.start()?;
        debug!("Device {} started", dev.ethdev.port_id());
        dev.running = true;
    }
    Ok(())
}

/// Stop all probed devices.
#[inline]
pub fn device_stop() -> Result<()> {
    let mut inet_device = INET_DEVICE.write().map_err(Error::from)?;
    let inet_iter = inet_device.iter_mut();
    for dev in inet_iter {
        dev.ethdev.stop()?;
        debug!("Device {} stopped", dev.ethdev.port_id());
        dev.running = false;
    }
    Ok(())
}

/// Close all probed device.
pub(crate) fn device_close() -> Result<()> {
    let mut inet_device = INET_DEVICE.write().map_err(Error::from)?;
    inet_device.clear();
    Ok(())
}

/// Get a device from an IP address.
pub(crate) fn find_dev_by_ip(ip: IpAddr) -> Result<(TxSender, rte_ether_addr)> {
    let inet_device = INET_DEVICE.read().map_err(Error::from)?;
    let inet_iter = inet_device.iter();
    for dev in inet_iter {
        #[allow(clippy::else_if_without_else)] // continue if not matched
        if dev.ip == ip {
            if !dev.running {
                error!("Device is not running!");
                return Err(Error::NoDev);
            }
            let sender = dev.ethdev.sender(0).ok_or(Error::NoDev)?;
            let addr = dev.ethdev.mac_addr()?;
            return Ok((sender, addr));
        } else if ip.is_unspecified() || ip.is_loopback() {
            if !dev.running {
                debug!("Device is not running, try the next one");
                continue;
            }
            let sender = dev.ethdev.sender(0).ok_or(Error::NoDev)?;
            let addr = dev.ethdev.mac_addr()?;
            return Ok((sender, addr));
        }
    }
    Err(Error::InvalidArg)
}
