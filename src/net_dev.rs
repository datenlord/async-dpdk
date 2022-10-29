//! Net device.

use dpdk_sys::{
    cstring, rte_eth_dev_info, rte_eth_dev_info_get, rte_ether_addr, rte_free, rte_malloc,
};
use lazy_static::lazy_static;
use std::{mem, net::IpAddr, os::raw::c_void, sync::RwLock};

use crate::{
    eth_dev::{EthDev, TxSender},
    Error, Result,
};

lazy_static! {
    static ref INET_DEVICE: RwLock<Vec<InetDevice>> = RwLock::new(Vec::default());
}

#[derive(Debug)]
struct InetDevice {
    ip: IpAddr,
    ethdev: EthDev,
    running: bool,
}

/// Probe all devices.
#[allow(unsafe_code)]
pub(crate) fn device_probe(addrs: Vec<IpAddr>) -> Result<()> {
    let mut inet_device = INET_DEVICE.write().unwrap();
    if !inet_device.is_empty() {
        return Err(Error::Already);
    }
    let ndev = EthDev::available_ports();
    if (ndev as usize) < addrs.len() {
        return Err(Error::InvalidArg);
    }
    let ndev = addrs.len().min(ndev as usize);
    for i in 0..ndev {
        let port_id = i as u16;
        let dev_info = unsafe {
            let dev_info = rte_malloc(
                cstring!("rte_eth_dev_info"),
                mem::size_of::<rte_eth_dev_info>(),
                0,
            );
            let errno = rte_eth_dev_info_get(port_id, dev_info.cast());
            Error::from_ret(errno)?;
            &mut *(dev_info as *mut rte_eth_dev_info)
        };
        let n_rxq = dev_info.max_rx_queues;
        let n_txq = dev_info.max_tx_queues;
        let ethdev = EthDev::new(port_id, n_rxq, n_txq)?;
        inet_device.push(InetDevice {
            ip: addrs[i],
            ethdev,
            running: false,
        });
        #[allow(trivial_casts)]
        unsafe {
            rte_free(dev_info as *mut rte_eth_dev_info as *mut c_void)
        };
    }
    Ok(())
}

/// Start all probed devices.
pub fn device_start() -> Result<()> {
    let mut inet_device = INET_DEVICE.write().unwrap();
    for dev in inet_device.iter_mut() {
        dev.ethdev.start()?;
        dev.running = true;
    }
    Ok(())
}

/// Stop all probed devices.
pub fn device_stop() -> Result<()> {
    let mut inet_device = INET_DEVICE.write().unwrap();
    for dev in inet_device.iter_mut() {
        dev.ethdev.stop()?;
        dev.running = false;
    }
    Ok(())
}

/// Close all probed device.
pub(crate) fn device_close() -> Result<()> {
    let mut inet_device = INET_DEVICE.write().unwrap();
    inet_device.clear();
    Ok(())
}

pub(crate) fn find_dev_by_ip(ip: IpAddr) -> Result<(TxSender, rte_ether_addr)> {
    let inet_device = INET_DEVICE.read().unwrap();
    for dev in inet_device.iter() {
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
