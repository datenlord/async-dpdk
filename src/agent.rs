//! RX/TX agent thread
use dpdk_sys::{
    rte_eth_rx_burst, rte_eth_tx_burst, rte_ether_hdr, rte_ipv4_hdr, rte_ipv6_hdr,
    RTE_ETHER_TYPE_ARP, RTE_ETHER_TYPE_IPV4, RTE_ETHER_TYPE_IPV6,
};
use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::ptr;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::{
    sync::mpsc,
    task::{self, JoinHandle},
};

use crate::mbuf::Mbuf;
use crate::protocol::IP_NEXT_PROTO_UDP;

use crate::udp::handle_ipv4_udp;

const MAX_PKT_BURST: usize = 32;
const TX_CHAN_SIZE: usize = 1024;

pub(crate) struct RxAgent {
    running: AtomicBool,
    tasks: Mutex<BTreeSet<(u16, u16)>>,
}

pub(crate) struct TxAgent {
    // TODO make a single-thread runtime to handle those tasks
    tasks: Mutex<BTreeMap<(u16, u16), JoinHandle<()>>>,
}

#[inline]
#[allow(unsafe_code)]
fn handle_ether(m: Mbuf) {
    // l3 protocol, l4 protocol
    let (ether_type, proto_id) = {
        let data = m.data_slice();
        let ether_hdr = unsafe { &*(data.as_ptr() as *const rte_ether_hdr) };
        let ether_type = ether_hdr.ether_type.to_be() as u32;

        let proto_id = match ether_type {
            RTE_ETHER_TYPE_IPV4 => {
                #[allow(trivial_casts)]
                let ip_hdr = unsafe {
                    &*((ether_hdr as *const rte_ether_hdr).add(1) as *const rte_ipv4_hdr)
                };
                ip_hdr.next_proto_id
            }
            RTE_ETHER_TYPE_IPV6 => {
                #[allow(trivial_casts)]
                let ip_hdr = unsafe {
                    &*((ether_hdr as *const rte_ether_hdr).add(1) as *const rte_ipv6_hdr)
                };
                ip_hdr.proto
            }
            RTE_ETHER_TYPE_ARP => 0,
            _ => 0,
        };

        (ether_type, proto_id)
    };

    match ether_type {
        RTE_ETHER_TYPE_IPV4 => match proto_id {
            IP_NEXT_PROTO_UDP => handle_ipv4_udp(m),
            _ => {}
        },
        RTE_ETHER_TYPE_IPV6 => {}
        RTE_ETHER_TYPE_ARP => {}
        ether_type => eprintln!("Unsupported ether type {ether_type:x}"),
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
                        handle_ether(m);
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
            tasks: Mutex::new(BTreeMap::new()),
        };
        Arc::new(this)
    }

    pub(crate) fn register(self: &Arc<Self>, port_id: u16, queue_id: u16) -> mpsc::Sender<Mbuf> {
        let (tx, mut rx) = mpsc::channel::<Mbuf>(TX_CHAN_SIZE);
        let handle = task::spawn(async move {
            while let Some(m) = rx.recv().await {
                let mut ptrs = vec![m.as_ptr()];
                mem::forget(m);
                // SAFETY: ffi
                // TODO handle unsent packets
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
