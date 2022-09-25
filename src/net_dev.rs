//! Net device.

use dpdk_sys::{cstring, rte_eth_dev_info, rte_eth_dev_info_get, rte_free, rte_malloc};
use lazy_static::lazy_static;
use std::{
    mem,
    net::{IpAddr, Ipv4Addr},
    os::raw::c_void,
    sync::{Arc, RwLock},
};

use crate::{
    eth_dev::{EthDev, EthRxQueue, EthTxQueue},
    Error, Result,
};

lazy_static! {
    static ref INET_DEVICE: RwLock<Vec<InetDevice>> = RwLock::new(Vec::default());
}

#[derive(Debug, Clone)]
struct InetDevice {
    ip: IpAddr,
    rx: Arc<EthRxQueue>,
    tx: Arc<EthTxQueue>,
}

/// Probe all device
#[allow(unsafe_code)]
pub fn device_probe() -> Result<()> {
    let mut inet_device = INET_DEVICE.write().unwrap();
    if !inet_device.is_empty() {
        return Err(Error::Already);
    }

    let ndev = EthDev::available_ports();
    for port_id in 0..ndev {
        let dev_info = unsafe {
            let dev_info = rte_malloc(
                cstring!("rte_eth_dev_info"),
                mem::size_of::<rte_eth_dev_info>(),
                0,
            );
            let errno = rte_eth_dev_info_get(port_id as _, dev_info.cast());
            Error::from_ret(errno)?;
            &mut *(dev_info as *mut rte_eth_dev_info)
        };
        let n_rxq = dev_info.max_rx_queues;
        let n_txq = dev_info.max_tx_queues;
        let nq = n_rxq.min(n_txq); // XXX: for convinience
        let dev = EthDev::new(port_id as _, n_rxq, n_txq)?;
        for queue_id in 0..nq {
            let rx = EthRxQueue::init(&dev, queue_id)?;
            let tx = EthTxQueue::init(&dev, queue_id)?;
            inet_device.push(InetDevice {
                ip: IpAddr::V4(Ipv4Addr::LOCALHOST), // TODO should choose a real ip
                rx,
                tx,
            })
        }
        #[allow(trivial_casts)]
        unsafe {
            rte_free(dev_info as *mut rte_eth_dev_info as *mut c_void)
        };
        dev.start()?;
    }
    Ok(())
}

pub(crate) fn find_dev_by_ip(ip: IpAddr) -> Result<(Arc<EthRxQueue>, Arc<EthTxQueue>)> {
    let inet_device = INET_DEVICE.read().unwrap();
    for dev in inet_device.iter() {
        if dev.ip == ip {
            return Ok((dev.rx.clone(), dev.tx.clone()));
        }
    }
    Err(Error::InvalidArg)
}
