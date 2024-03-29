//! Net device.

use crate::{
    eth_dev::{EthDev, TxSender},
    Error, Result,
};
use dpdk_sys::{rte_eth_dev_info, rte_eth_dev_info_get, rte_ether_addr, rte_free, rte_malloc};
use lazy_static::lazy_static;
use log::{debug, error};
use std::{ffi::CString, mem, net::IpAddr, sync::RwLock};

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
///
/// IP addresses assigned to devices should be distinct. The input addresses
/// are automatically deduplicated.
#[allow(unsafe_code)]
#[allow(clippy::similar_names)] // tx and rx are DPDK terms
pub(crate) fn device_probe(mut addrs: Vec<IpAddr>, max_queues: u16) -> Result<()> {
    let mut inet_device = INET_DEVICE.write().map_err(Error::from)?;
    if !inet_device.is_empty() {
        error!("Device already probed");
        return Err(Error::Already);
    }
    addrs.dedup();
    let ndev = EthDev::available_ports();
    if (ndev as usize) < addrs.len() || (u16::MAX as usize) < addrs.len() {
        error!("Address list too long");
        return Err(Error::InvalidArg);
    }
    for (i, addr) in addrs.into_iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)] // checked
        let port_id = i as u16;
        // SAFETY: `dev_info` validity checked in `rte_eth_dev_info_get`
        let dev_info = unsafe {
            let name = CString::new("rte_eth_dev_info").map_err(Error::from)?;
            let dev_info = rte_malloc(name.as_ptr(), mem::size_of::<rte_eth_dev_info>(), 0);
            let errno = rte_eth_dev_info_get(port_id, dev_info.cast());
            Error::from_ret(errno).map_err(|e| {
                rte_free(dev_info.cast());
                e
            })?;
            &mut *(dev_info.cast::<rte_eth_dev_info>())
        };
        let n_rxq = dev_info.max_rx_queues.min(max_queues);
        let n_txq = dev_info.max_tx_queues.min(max_queues);
        let ethdev = EthDev::new(port_id, n_rxq, n_txq)?;
        inet_device.push(InetDevice {
            ip: addr,
            ethdev,
            running: false,
        });
        debug!("Ethdev {port_id} probed, bound to {addr:?}");
        // SAFETY: dev_info`'s validity is checked upon its allocation
        #[allow(trivial_casts)]
        unsafe {
            rte_free((dev_info as *mut rte_eth_dev_info).cast());
        }
    }
    Ok(())
}

/// Start all probed devices.
///
/// # Errors
///
/// - Lock poisoned.
/// - Unable to start device.
#[inline]
pub fn device_start_all() -> Result<()> {
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
///
/// # Errors
///
/// Possible reasons:
///
/// - Lock poisoned.
/// - Unable to stop device.
#[inline]
pub fn device_stop_all() -> Result<()> {
    let mut inet_device = INET_DEVICE.write().map_err(Error::from)?;
    let inet_iter = inet_device.iter_mut();
    for dev in inet_iter {
        dev.ethdev.stop()?;
        debug!("Device {} stopped", dev.ethdev.port_id());
        dev.running = false;
    }
    Ok(())
}

/// Start a specific probed device.
///
/// # Errors
///
/// Possible reasons:
///
/// - Lock poisoned.
/// - Unable to start device.
#[inline]
pub fn device_start(addr: &IpAddr) -> Result<()> {
    let mut inet_device = INET_DEVICE.write().map_err(Error::from)?;
    let inet_iter = inet_device.iter_mut();
    for dev in inet_iter {
        if &dev.ip == addr {
            dev.ethdev.start()?;
            debug!("Device {} started", dev.ethdev.port_id());
            dev.running = true;
            return Ok(());
        }
    }
    Err(Error::NoDev)
}

/// Close a specific probed device.
///
/// # Errors
///
/// Possible reasons:
///
/// - Lock poisoned.
/// - Unable to stop device.
#[inline]
pub fn device_stop(addr: &IpAddr) -> Result<()> {
    let mut inet_device = INET_DEVICE.write().map_err(Error::from)?;
    let inet_iter = inet_device.iter_mut();
    for dev in inet_iter {
        if &dev.ip == addr {
            dev.ethdev.stop()?;
            debug!("Device {} stopped", dev.ethdev.port_id());
            dev.running = false;
            return Ok(());
        }
    }
    Err(Error::NoDev)
}

/// Close all probed device.
pub(crate) fn device_close() -> Result<()> {
    let mut inet_device = INET_DEVICE.write().map_err(Error::from)?;
    inet_device.clear();
    Ok(())
}

/// Get a device from an IP address.
///
/// The returned result will be a tuple of a `TxSender` sending messages to that device and its Ether
/// address.
pub(crate) fn find_dev_by_ip(ip: IpAddr) -> Result<(TxSender, rte_ether_addr)> {
    let inet_device = INET_DEVICE.read().map_err(Error::from)?;
    let inet_iter = inet_device.iter();
    for dev in inet_iter {
        if dev.ip == ip {
            if !dev.running {
                error!("Device is not running!");
                return Err(Error::NoDev);
            }
            let sender = dev.ethdev.sender(0).ok_or(Error::NotStart)?;
            let addr = dev.ethdev.mac_addr()?;
            return Ok((sender, addr));
        }
        if ip.is_unspecified() || ip.is_loopback() {
            if !dev.running {
                debug!("Device is not running, try the next one");
                continue;
            }
            let sender = dev.ethdev.sender(0).ok_or(Error::NotStart)?;
            let addr = dev.ethdev.mac_addr()?;
            return Ok((sender, addr));
        }
    }
    error!("Ip address {ip} not matched to any address");
    Err(Error::InvalidArg)
}
