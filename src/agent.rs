//! RX/TX agent thread
#[allow(clippy::wildcard_imports)] // too many of them
use dpdk_sys::*;
use std::collections::{BTreeMap, BTreeSet};
use std::mem;
use std::ptr::{self, NonNull};
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
use crate::proto::{ETHER_HDR_LEN, IP_NEXT_PROTO_UDP};
use crate::udp::handle_ipv4_udp;
use crate::{Error, Result};

/// Burst size for `rte_tx_burst` and `rte_rx_burst`.
const MAX_PKT_BURST: u16 = 32;
/// Channel size for `TxAgent`.
const TX_CHAN_SIZE: usize = 256;
/// The capacity of a `TxBuffer`.
const TX_BUF_SIZE: usize = 1024;
/// The number of mbufs to flush a `TxBuffer`.
#[allow(dead_code)]
const TX_BUF_THRESH: usize = 8;

/// Number of buckets in the hash table.
const IP_FRAG_TABLE_BUCKET_NUM: u32 = 128;
/// Number of entries per bucket (e.g. hash associativity). Should be power of two.
const IP_FRAG_TABLE_BUCKET_SIZE: u32 = 16;
/// Maximum number of entries that could be stored in the table. The value should be less
/// or equal then `bucket_num` * `bucket_entries`.
const IP_FRAG_TABLE_MAX_ENTRIES: u32 = 2048;

/// An agent thread doing receiving.
pub(crate) struct RxAgent {
    /// A bool indicating whether the thread is running.
    running: AtomicBool,
    /// A set of (`port_id`, `queue_id`) to be polled.
    tasks: Mutex<BTreeSet<(u16, u16)>>,
}

/// An agent thread doing sending.
pub(crate) struct TxAgent {
    /// Single-threaded Runtime.
    rt: Option<Runtime>,
    /// (`port_id`, `queue_id`) -> Task
    tasks: Mutex<BTreeMap<(u16, u16), JoinHandle<()>>>,
}

/// Table holding fragmented packets.
struct IpFragmentTable {
    /// `rte_ip_frag_tbl` pointer.
    tbl: NonNull<rte_ip_frag_tbl>,
}

/// Table holding packets to be deallocated.
struct IpFragDeathRow {
    /// `rte_ip_frag_death_row` pointer.
    dr: NonNull<rte_ip_frag_death_row>,
}

#[allow(unsafe_code)]
#[allow(clippy::missing_docs_in_private_items)]
impl IpFragmentTable {
    fn new(socket_id: u32) -> Result<Self> {
        const MS_PER_S: u64 = 1000;
        // SAFETY: ffi
        let max_cycles = unsafe { (rte_get_tsc_hz() + MS_PER_S - 1) / MS_PER_S * MS_PER_S };
        // SAFETY: ffi
        let ptr = unsafe {
            rte_ip_frag_table_create(
                IP_FRAG_TABLE_BUCKET_NUM,
                IP_FRAG_TABLE_BUCKET_SIZE,
                IP_FRAG_TABLE_MAX_ENTRIES,
                max_cycles,
                socket_id as _,
            )
        };
        let tbl = NonNull::new(ptr).ok_or(Error::NoMem)?;
        Ok(Self { tbl })
    }
    fn as_mut_ptr(&mut self) -> *mut rte_ip_frag_tbl {
        self.tbl.as_ptr()
    }
}

#[allow(unsafe_code)]
impl Drop for IpFragmentTable {
    fn drop(&mut self) {
        // SAFETY: ffi
        unsafe { rte_ip_frag_table_destroy(self.as_mut_ptr()) };
    }
}

#[allow(unsafe_code)]
#[allow(clippy::missing_docs_in_private_items)]
impl IpFragDeathRow {
    fn new(socket_id: u32) -> Result<Self> {
        // SAFETY: ffi, check not null later.
        let ptr = unsafe {
            rte_zmalloc_socket(
                cstring!("death_row"),
                mem::size_of::<rte_ip_frag_death_row>(),
                0,
                socket_id as _,
            )
            .cast::<rte_ip_frag_death_row>()
        };
        let dr = NonNull::new(ptr).ok_or(Error::NoMem)?;
        Ok(Self { dr })
    }
    fn as_mut_ptr(&mut self) -> *mut rte_ip_frag_death_row {
        self.dr.as_ptr()
    }
}

#[allow(unsafe_code)]
impl Drop for IpFragDeathRow {
    fn drop(&mut self) {
        // SAFETY: ffi
        unsafe {
            rte_free(self.as_mut_ptr().cast());
        }
    }
}

/// Check Ethernet packet validity and return its L3 protocol and L4 protocol.
#[inline]
#[allow(unsafe_code)]
fn parse_ether(m: &Mbuf) -> Option<(u32, u8)> {
    let data = m.data_slice();
    if data.len() < ETHER_HDR_LEN {
        return None;
    }
    let remain = data.len() - ETHER_HDR_LEN;
    // SAFETY: mbuf data size is greater than `rte_ether_hdr` size
    #[allow(clippy::cast_ptr_alignment)]
    let ether_hdr = unsafe { &*(data.as_ptr().cast::<rte_ether_hdr>()) };
    let ether_type = u32::from(ether_hdr.ether_type.to_be());

    match ether_type {
        RTE_GTP_TYPE_IPV4 => {
            if remain < mem::size_of::<rte_ipv4_hdr>() {
                return None;
            }
        }
        RTE_GTP_TYPE_IPV6 => {
            if remain < mem::size_of::<rte_ipv6_hdr>() {
                return None;
            }
        }
        _ => return None,
    }
    let proto_id = match ether_type {
        RTE_ETHER_TYPE_IPV4 => {
            // SAFETY: set bitfields
            unsafe {
                let pm = &mut *(m.as_ptr());
                pm.tx_offload_union
                    .tx_offload_struct
                    .set_l3_len(mem::size_of::<rte_ipv4_hdr>() as _);
            }
            // SAFETY: remain mbuf data size is greater than `rte_ipv4_hdr` size
            #[allow(trivial_casts)]
            let ip_hdr = unsafe {
                &*((ether_hdr as *const rte_ether_hdr)
                    .add(1)
                    .cast::<rte_ipv4_hdr>())
            };
            ip_hdr.next_proto_id
        }
        RTE_ETHER_TYPE_IPV6 => {
            // SAFETY: set bitfields
            unsafe {
                let pm = &mut *(m.as_ptr());
                pm.tx_offload_union
                    .tx_offload_struct
                    .set_l3_len(mem::size_of::<rte_ipv6_hdr>() as _);
            }
            // SAFETY: remain mbuf data size is greater than `rte_ipv6_hdr` size
            #[allow(trivial_casts)]
            let ip_hdr = unsafe {
                &*((ether_hdr as *const rte_ether_hdr)
                    .add(1)
                    .cast::<rte_ipv6_hdr>())
            };
            ip_hdr.proto
        }
        _ => 0,
    };
    Some((ether_type, proto_id))
}

/// Handle L2 frame.
#[inline]
#[allow(unsafe_code)]
fn handle_ether(mut m: Mbuf, tbl: &mut IpFragmentTable, dr: &mut IpFragDeathRow) {
    // l3 protocol, l4 protocol
    if let Some((ether_type, proto_id)) = parse_ether(&m) {
        #[allow(clippy::unwrap_used)]
        m.adj(ETHER_HDR_LEN).unwrap();
        match ether_type {
            RTE_ETHER_TYPE_IPV4 => {
                let ptr = m.data_slice_mut().as_mut_ptr();
                // SAFETY: ffi
                let new_m = if unsafe { rte_ipv4_frag_pkt_is_fragmented(ptr.cast()) } == 0 {
                    Some(m)
                } else {
                    // SAFETY: ffi
                    let mo = unsafe {
                        let tms = rte_rdtsc();
                        rte_ipv4_frag_reassemble_packet(
                            tbl.as_mut_ptr(),
                            dr.as_mut_ptr(),
                            m.as_ptr(),
                            tms,
                            ptr.cast(),
                        )
                    };
                    if mo.is_null() {
                        mem::forget(m);
                        None // in need of more fragments
                    } else if mo != m.as_ptr() {
                        mem::forget(m);
                        #[allow(clippy::shadow_unrelated, clippy::unwrap_used)] // they're related
                        let m = Mbuf::new_with_ptr(mo).unwrap();
                        Some(m)
                    } else {
                        Some(m)
                    }
                };
                if new_m.is_none() {
                    return;
                }
                #[allow(clippy::shadow_unrelated, clippy::unwrap_used)] // they're related
                let m = new_m.unwrap();
                #[allow(clippy::single_match)] // more protocols
                match proto_id {
                    IP_NEXT_PROTO_UDP => handle_ipv4_udp(m),
                    _ => {}
                }
            }
            RTE_ETHER_TYPE_IPV6 | RTE_ETHER_TYPE_ARP => {}
            ether_type => eprintln!("Unsupported ether type {ether_type:x}"),
        }
    }
}

#[allow(unsafe_code)]
impl RxAgent {
    /// Start an `RxAgent`, spawn a thread to do the polling job.
    pub(crate) fn start(socket_id: u32) -> Arc<Self> {
        let running = AtomicBool::new(true);
        let this = Arc::new(RxAgent {
            running,
            tasks: Mutex::new(BTreeSet::new()),
        });
        let that = Arc::clone(&this);
        let _handle = task::spawn_blocking(move || {
            #[allow(clippy::unwrap_used)]
            let mut frag_tbl = IpFragmentTable::new(socket_id).unwrap();
            #[allow(clippy::unwrap_used)]
            let mut death_row = IpFragDeathRow::new(socket_id).unwrap();
            // TODO error handling
            while that.running.load(Ordering::Acquire) {
                #[allow(clippy::unwrap_used)]
                let tasks = that.tasks.lock().unwrap();
                let task_iter = tasks.iter();
                for &(port_id, queue_id) in task_iter {
                    let mut ptrs = vec![ptr::null_mut(); MAX_PKT_BURST as usize];
                    // SAFETY: ffi
                    let n = unsafe {
                        rte_eth_rx_burst(port_id, queue_id, ptrs.as_mut_ptr(), MAX_PKT_BURST)
                    };
                    for ptr in ptrs.into_iter().take(n as _) {
                        #[allow(clippy::unwrap_used)]
                        let m = Mbuf::new_with_ptr(ptr).unwrap();
                        handle_ether(m, &mut frag_tbl, &mut death_row);
                    }
                }
            }
        });
        this
    }

    /// Stop the `RxAgent`.
    pub(crate) fn stop(self: &Arc<Self>) {
        self.running.store(false, Ordering::Release);
    }

    /// Register a (`port_id`, `queue_id`) to an `RxAgent`.
    pub(crate) fn register(self: &Arc<Self>, port_id: u16, queue_id: u16) {
        #[allow(clippy::unwrap_used)]
        let _ = self.tasks.lock().unwrap().insert((port_id, queue_id));
    }

    /// Unregister a (`port_id`, `queue_id`) from an `RxAgent`.
    pub(crate) fn unregister(self: &Arc<Self>, port_id: u16, queue_id: u16) {
        #[allow(clippy::unwrap_used)]
        let _ = self.tasks.lock().unwrap().remove(&(port_id, queue_id));
    }
}

#[allow(unsafe_code)]
impl TxAgent {
    /// Start a `TxBuffer`, spawn a thread to do the sending job.
    pub(crate) fn start() -> Arc<Self> {
        #[allow(clippy::unwrap_used)]
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

    /// Register a (`port_id`, `queue_id`) to a `TxAgent`.
    pub(crate) fn register(self: &Arc<Self>, port_id: u16, queue_id: u16) -> mpsc::Sender<Mbuf> {
        let (tx, mut rx) = mpsc::channel::<Mbuf>(TX_CHAN_SIZE);
        #[allow(clippy::unwrap_used)]
        let handle = self.rt.as_ref().unwrap().spawn(async move {
            let mut txbuf = TxBuffer::new(port_id, queue_id);
            while let Some(m) = rx.recv().await {
                #[allow(clippy::unwrap_used)]
                txbuf.buffer(m).unwrap();
            }
        });
        #[allow(clippy::unwrap_used)]
        let _prev = self
            .tasks
            .lock()
            .unwrap()
            .insert((port_id, queue_id), handle);
        tx
    }

    /// Unregister a (`port_id`, `queue_id`) from a `TxAgent`.
    pub(crate) fn unregister(self: &Arc<Self>, port_id: u16, queue_id: u16) {
        #[allow(clippy::unwrap_used)]
        if let Some(handle) = self.tasks.lock().unwrap().remove(&(port_id, queue_id)) {
            handle.abort();
        }
    }
}

impl Drop for TxAgent {
    fn drop(&mut self) {
        #[allow(clippy::unwrap_used)]
        let rt = self.rt.take().unwrap();
        mem::forget(rt);
    }
}

/// `TxBuffer` holding unsent mbufs.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
struct TxBuffer {
    /// Start index of mbufs.
    start: usize,
    /// End index of mbufs.
    end: usize,
    /// Total number of mbufs held.
    size: usize,
    /// Total capacity of this buffer.
    capacity: usize,
    /// `port_id` that the mbufs are sent to.
    port_id: u16,
    /// `queue_id` that the mbufs are sent to.
    queue_id: u16,
    /// `mbuf`s held.
    mbufs: [*mut rte_mbuf; TX_BUF_SIZE],
}

// SAFETY: `TxBuffer` is `Send`.
#[allow(unsafe_code)]
unsafe impl Send for TxBuffer {}

#[allow(unsafe_code)]
impl TxBuffer {
    /// Allocate a `TxBuffer` on the given port and queue.
    fn new(port_id: u16, queue_id: u16) -> Self {
        let mbufs = [ptr::null_mut(); TX_BUF_SIZE];
        Self {
            start: 0,
            end: 0,
            size: 0,
            capacity: TX_BUF_SIZE,
            port_id,
            queue_id,
            mbufs,
        }
    }

    /// Populate the fragmented IP packets.
    #[inline]
    fn populate_ether_hdr(ether_src: &rte_ether_hdr, mbufs: &[*mut rte_mbuf]) {
        for &m in mbufs {
            // SAFETY: ffi
            unsafe {
                #[allow(clippy::cast_ptr_alignment)]
                let ether_dst = rte_pktmbuf_prepend(m, ETHER_HDR_LEN as _).cast::<rte_ether_hdr>();
                (*ether_dst).ether_type = ether_src.ether_type;
                rte_ether_addr_copy(&(*ether_src).src_addr, &mut (*ether_dst).src_addr);
                rte_ether_addr_copy(&(*ether_src).dst_addr, &mut (*ether_dst).dst_addr);
            }
        }
    }

    /// Send any packets queued up for transmission on a port and HW queue.
    #[inline]
    fn buffer(&mut self, m: Mbuf) -> Result<()> {
        // Put the new mbuf at the end of buffer.
        if m.pkt_len() < RTE_ETHER_MTU as usize {
            if self.capacity - self.size < 1 {
                return Err(Error::NoBuf);
            }
            #[allow(clippy::indexing_slicing)]
            self.mbufs[self.end] = m.as_ptr();
            self.end = (self.end + 1) % self.capacity;
            self.size += 1;
            mem::forget(m);
        } else {
            // need fragment
            let nb_frags = m.pkt_len() / RTE_ETHER_MTU as usize + 1;
            if self.capacity - self.size < nb_frags + 1 {
                return Err(Error::NoBuf);
            }
            let mut frags: Vec<*mut rte_mbuf> = vec![ptr::null_mut(); nb_frags];
            let pm = m.as_ptr();
            // SAFETY: ffi, data length > size of `rte_ether_hdr`
            #[allow(clippy::cast_ptr_alignment)]
            let ether_src = unsafe {
                &*(rte_mbuf_buf_addr(pm, (*pm).pool).add((*pm).data_off as _)
                    as *const rte_ether_hdr)
            };
            // SAFETY: ffi
            let _ = unsafe { rte_pktmbuf_adj(pm, ETHER_HDR_LEN as _) };
            // SAFETY: ffi
            let errno = unsafe {
                let l3_type = (*pm).packet_type_union.packet_type & RTE_PTYPE_L3_MASK;
                if l3_type == RTE_PTYPE_L3_IPV4 {
                    rte_ipv4_fragment_packet(
                        pm,
                        frags.as_mut_ptr(),
                        nb_frags as _,
                        RTE_ETHER_MTU as _,
                        (*pm).pool,
                        (*pm).pool,
                    )
                } else if l3_type == RTE_PTYPE_L3_IPV6 {
                    rte_ipv6_fragment_packet(
                        pm,
                        frags.as_mut_ptr(),
                        nb_frags as _,
                        RTE_ETHER_MTU as _,
                        (*pm).pool,
                        (*pm).pool,
                    )
                } else {
                    -1
                }
            };
            Error::from_ret(errno)?;
            #[allow(clippy::shadow_unrelated, clippy::cast_sign_loss)]
            // they're related actually, errno is greater than 0
            let nb_frags = errno as usize;
            #[allow(clippy::indexing_slicing)]
            Self::populate_ether_hdr(ether_src, &frags[..nb_frags]);
            for (i, mb) in frags.iter().enumerate() {
                let idest = (self.end + i) % self.capacity;
                #[allow(clippy::indexing_slicing)]
                self.mbufs[idest] = *mb;
            }
            self.end = (self.end + nb_frags) % self.capacity;
            self.size += nb_frags;
        }

        if self.start < self.end {
            let to_send = (self.end - self.start) as u16;
            // if to_send < TX_BUF_THRESH {
            //     return;
            // }
            // SAFETY: ffi
            let sent = unsafe {
                rte_eth_tx_burst(
                    self.port_id,
                    self.queue_id,
                    self.mbufs.as_mut_ptr().add(self.start),
                    to_send,
                )
            };
            self.start += sent as usize;
        } else {
            let to_send1 = (self.capacity - self.start) as u16;
            let to_send2 = self.end as u16;
            // if to_send1 + to_send2 < TX_BUF_THRESH {
            //     return;
            // }
            // SAFETY: ffi
            let mut sent = unsafe {
                rte_eth_tx_burst(
                    self.port_id,
                    self.queue_id,
                    self.mbufs.as_mut_ptr().add(self.start),
                    to_send1,
                )
            };
            // SAFETY: ffi
            sent += unsafe {
                rte_eth_tx_burst(
                    self.port_id,
                    self.queue_id,
                    self.mbufs.as_mut_ptr(),
                    to_send2,
                )
            };
            self.start = self.start.wrapping_add(sent as _) % self.capacity;
        }
        Ok(())
    }
}
