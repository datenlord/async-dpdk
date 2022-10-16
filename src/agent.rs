//! RX/TX agent thread
use dpdk_sys::{
    rte_eth_rx_burst, rte_eth_tx_burst, rte_ether_hdr, rte_ipv4_hdr, rte_ipv6_hdr, rte_mbuf,
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
    runtime::{Builder, Runtime},
    sync::mpsc,
    task::{self, JoinHandle},
};

use crate::mbuf::Mbuf;
use crate::protocol::IP_NEXT_PROTO_UDP;
use crate::udp::{handle_ipv4_udp, handle_ipv6_udp};

const MAX_PKT_BURST: usize = 32;
const TX_CHAN_SIZE: usize = 256;
const TX_BUF_SIZE: usize = 1024;
#[allow(dead_code)]
const TX_BUF_THRESH: usize = 8;

pub(crate) struct RxAgent {
    running: AtomicBool,
    tasks: Mutex<BTreeSet<(u16, u16)>>,
}

pub(crate) struct TxAgent {
    rt: Option<Runtime>,
    tasks: Mutex<BTreeMap<(u16, u16), JoinHandle<()>>>,
}

#[inline]
#[allow(unsafe_code)]
fn handle_ether(m: Mbuf) {
    // TODO error handling
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
        RTE_ETHER_TYPE_IPV6 => match proto_id {
            IP_NEXT_PROTO_UDP => handle_ipv6_udp(m),
            _ => {}
        },
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
            // TODO error handling
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
        // XXX start a background single-thread async runtime
        let rt = Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("dpdk-tx-agent")
            .build()
            .unwrap();
        let this = TxAgent {
            rt: Some(rt),
            tasks: Mutex::new(BTreeMap::new()),
        };
        Arc::new(this)
    }

    pub(crate) fn register(self: &Arc<Self>, port_id: u16, queue_id: u16) -> mpsc::Sender<Mbuf> {
        let (tx, mut rx) = mpsc::channel::<Mbuf>(TX_CHAN_SIZE);
        let handle = self.rt.as_ref().unwrap().spawn(async move {
            let mut txbuf = TxBuffer::new(port_id, queue_id);
            while let Some(m) = rx.recv().await {
                txbuf.buffer(m);
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

impl Drop for TxAgent {
    fn drop(&mut self) {
        let rt = self.rt.take().unwrap();
        mem::forget(rt);
    }
}

#[allow(missing_copy_implementations)]
#[derive(Debug)]
struct TxBuffer {
    start: usize,
    end: usize,
    size: usize,
    port_id: u16,
    queue_id: u16,
    mbufs: [*mut rte_mbuf; TX_BUF_SIZE],
}

// SAFETY: not sure whether TxBuffer is `Send`, it's used on a single-thread runtime.
#[allow(unsafe_code)]
unsafe impl Send for TxBuffer {}

#[allow(unsafe_code)]
impl TxBuffer {
    /// Allocate a TxBuffer on the given port and queue.
    fn new(port_id: u16, queue_id: u16) -> Self {
        let mbufs = [ptr::null_mut(); TX_BUF_SIZE];
        Self {
            start: 0,
            end: 0,
            size: TX_BUF_SIZE,
            port_id,
            queue_id,
            mbufs,
        }
    }

    /// Send any packets queued up for transmission on a port and HW queue.
    #[inline]
    fn buffer(&mut self, m: Mbuf) {
        // Put the new mbuf at the end of buffer.
        // XXX no remaining space to hold the mbuf
        self.mbufs[self.end] = m.as_ptr();
        self.end = (self.end + 1) % self.size;
        mem::forget(m);

        // The buffer is empty.
        if self.start == self.end {
            return;
        }
        if self.start < self.end {
            let to_send = self.end - self.start;
            // if to_send < TX_BUF_THRESH {
            //     return;
            // }
            let sent = unsafe {
                rte_eth_tx_burst(
                    self.port_id,
                    self.queue_id,
                    self.mbufs.as_mut_ptr().add(self.start),
                    to_send as _,
                )
            };
            self.start += sent as usize;
        } else {
            let to_send1 = self.size - self.start;
            let to_send2 = self.end;
            // if to_send1 + to_send2 < TX_BUF_THRESH {
            //     return;
            // }
            let mut sent = unsafe {
                rte_eth_tx_burst(
                    self.port_id,
                    self.queue_id,
                    self.mbufs.as_mut_ptr().add(self.start),
                    to_send1 as _,
                )
            };
            sent += unsafe {
                rte_eth_tx_burst(
                    self.port_id,
                    self.queue_id,
                    self.mbufs.as_mut_ptr(),
                    to_send2 as _,
                )
            };
            println!("sent {sent}");
            self.start = self.start.wrapping_add(sent as _) % self.size;
        }
    }
}
