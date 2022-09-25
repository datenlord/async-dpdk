//! RX/TX agent thread
use dpdk_sys::{
    rte_eth_rx_burst, rte_eth_tx_burst, rte_ether_hdr, rte_ipv4_hdr, rte_udp_hdr,
    RTE_ETHER_TYPE_IPV4,
};
use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::net::{IpAddr, SocketAddr};
use std::ptr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::{
    sync::mpsc,
    task::{self, JoinHandle, LocalSet},
};

use crate::mbuf::Mbuf;
use crate::packet::GenericPacket;
use crate::protocol::Packet;
use crate::socket::{self, addr_2_sockfd};

const MAX_PKT_BURST: usize = 32;
const TX_CHAN_SIZE: usize = 1024;

pub(crate) struct RxAgent {
    running: AtomicBool,
    tasks: Mutex<BTreeSet<(u16, u16)>>,
}

pub(crate) struct TxAgent {
    tasks: Mutex<BTreeMap<(u16, u16), JoinHandle<()>>>,
    local_set: LocalSet,
}

#[allow(unsafe_code)]
fn classify(m: Mbuf) {
    let data = m.data_slice();
    let ether_hdr = unsafe { &*(data.as_ptr() as *const rte_ether_hdr) };
    match ether_hdr.ether_type.to_le() as u32 {
        RTE_ETHER_TYPE_IPV4 => {
            #[allow(trivial_casts)]
            let ip_hdr =
                unsafe { &*((ether_hdr as *const rte_ether_hdr).add(1) as *const rte_ipv4_hdr) };
            let dst_ip_bytes: [u8; 4] = ip_hdr.dst_addr.to_le_bytes(); // XXX little endian?
            let dst_ip = IpAddr::from(dst_ip_bytes);
            let src_ip_bytes: [u8; 4] = ip_hdr.src_addr.to_le_bytes(); // XXX little endian?
            let src_ip = IpAddr::from(src_ip_bytes);

            #[allow(trivial_casts)]
            let udp_hdr =
                unsafe { &*((ip_hdr as *const rte_ipv4_hdr).add(1) as *const rte_udp_hdr) };
            let dst_port = udp_hdr.dst_port;
            let src_port = udp_hdr.src_port;
            let src_addr = SocketAddr::new(src_ip, src_port);

            let packet = GenericPacket::from_mbuf(m).unwrap();
            if let Some(sockfd) = addr_2_sockfd(dst_port, dst_ip) {
                socket::put_mailbox(sockfd, src_addr, packet); // XXX ???
            }
        }
        _ => unimplemented!(),
    }
}

#[allow(unsafe_code)]
impl RxAgent {
    pub(crate) fn start() -> Arc<Self> {
        let running = AtomicBool::new(true);
        let this = Arc::new(RxAgent {
            running,
            tasks: Mutex::new(BTreeSet::new()),
        });
        let that = this.clone();
        let _ = task::spawn_blocking(move || {
            while that.running.load(Ordering::Acquire) {
                let tasks = that.tasks.lock().unwrap();
                for &(port_id, queue_id) in tasks.iter() {
                    let mut ptrs = vec![ptr::null_mut(); MAX_PKT_BURST];
                    // SAFETY: ffi
                    let n = unsafe {
                        rte_eth_rx_burst(port_id, queue_id, ptrs.as_mut_ptr(), MAX_PKT_BURST as _)
                    };
                    for i in 0..n as usize {
                        let m = Mbuf::new_with_ptr(ptrs[i]).unwrap();
                        classify(m);
                    }
                }
            }
        });
        this
    }

    pub(crate) fn stop(self: &Arc<Self>) {
        self.running.store(false, Ordering::Release);
    }

    pub(crate) fn register(self: &Arc<Self>, port_id: u16, queue_id: u16) {
        let _ = self.tasks.lock().unwrap().insert((port_id, queue_id));
    }

    pub(crate) fn unregister(self: &Arc<Self>, port_id: u16, queue_id: u16) {
        let _ = self.tasks.lock().unwrap().remove(&(port_id, queue_id));
    }
}

#[allow(unsafe_code)]
impl TxAgent {
    pub(crate) fn start() -> Arc<Self> {
        let this = TxAgent {
            local_set: LocalSet::new(),
            tasks: Mutex::new(BTreeMap::new()),
        };
        Arc::new(this)
    }

    pub(crate) fn register(self: &Arc<Self>, port_id: u16, queue_id: u16) -> mpsc::Sender<Mbuf> {
        let (tx, mut rx) = mpsc::channel::<Mbuf>(TX_CHAN_SIZE);
        let handle = self.local_set.spawn_local(async move {
            while let Some(m) = rx.recv().await {
                let mut ptrs = vec![m.as_ptr()];
                mem::forget(m);
                // SAFETY: ffi
                let _n = unsafe {
                    rte_eth_tx_burst(port_id, queue_id, ptrs.as_mut_ptr(), ptrs.len() as _)
                };
            }
        });
        let _ = self
            .tasks
            .lock()
            .unwrap()
            .insert((port_id, queue_id), handle);
        tx
    }

    pub(crate) fn unregister(self: &Arc<Self>, port_id: u16, queue_id: u16) {
        if let Some(handle) = self.tasks.lock().unwrap().remove(&(port_id, queue_id)) {
            handle.abort();
        }
    }
}
