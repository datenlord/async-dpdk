//! RX/TX agent thread
use crate::mbuf::Mbuf;
use crate::proto::{L3Protocol, Protocol, ETHER_HDR_LEN, IP_NEXT_PROTO_UDP};
use crate::socket::{self, RecvResult};
use crate::udp::handle_ipv4_udp;
use crate::{Error, Result};
#[allow(clippy::wildcard_imports)] // too many of them
use dpdk_sys::*;
use log::{debug, error, info, trace, warn};
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

/// Burst size for `rte_tx_burst` and `rte_rx_burst`.
const MAX_PKT_BURST: u16 = 32;
/// Channel size for `TxAgent`.
const TX_CHAN_SIZE: usize = 256;
/// The capacity of a `TxBuffer`.
const TX_BUF_SIZE: u16 = 1024;
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
impl IpFragmentTable {
    /// Create an `IpFragmentTable`.
    fn new(socket_id: i32) -> Result<Self> {
        /// Millisecond per second.
        const MS_PER_S: u64 = 1000;
        // SAFETY: ffi
        let max_cycles = unsafe {
            rte_get_tsc_hz()
                .saturating_add(MS_PER_S.saturating_sub(1))
                .saturating_div(MS_PER_S.saturating_pow(2))
        };
        // SAFETY: ffi
        let ptr = unsafe {
            rte_ip_frag_table_create(
                IP_FRAG_TABLE_BUCKET_NUM,
                IP_FRAG_TABLE_BUCKET_SIZE,
                IP_FRAG_TABLE_MAX_ENTRIES,
                max_cycles,
                socket_id,
            )
        };
        let tbl = NonNull::new(ptr).ok_or(Error::NoMem)?;
        Ok(Self { tbl })
    }
    /// Get *mut `rte_ip_frag_tbl`.
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
impl IpFragDeathRow {
    /// Create a new `IpFragDeathRow`.
    fn new(socket_id: i32) -> Result<Self> {
        // SAFETY: ffi, check not null later.
        let ptr = unsafe {
            rte_zmalloc_socket(
                cstring!("death_row"),
                mem::size_of::<rte_ip_frag_death_row>(),
                0,
                socket_id,
            )
            .cast::<rte_ip_frag_death_row>()
        };
        let dr = NonNull::new(ptr).ok_or(Error::NoMem)?;
        Ok(Self { dr })
    }
    /// Get *mut `rte_ip_frag_death_row`.
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
    if data.len() < ETHER_HDR_LEN as usize {
        return None;
    }
    let remain = data.len().wrapping_sub(ETHER_HDR_LEN as _);
    // SAFETY: mbuf data size is greater than `rte_ether_hdr` size
    #[allow(clippy::cast_ptr_alignment)]
    let ether_hdr = unsafe { &*(data.as_ptr().cast::<rte_ether_hdr>()) };
    let ether_type = u32::from(ether_hdr.ether_type.to_be());

    match ether_type {
        RTE_ETHER_TYPE_IPV4 => {
            if remain < mem::size_of::<rte_ipv4_hdr>() {
                warn!("Receive a unexpectedly short IPv4 packet");
                return None;
            }
        }
        RTE_ETHER_TYPE_IPV6 => {
            if remain < mem::size_of::<rte_ipv6_hdr>() {
                warn!("Receive a unexpectedly short IPv6 packet");
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
                    .set_l3_len(L3Protocol::Ipv4.length());
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
                    .set_l3_len(L3Protocol::Ipv6.length());
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
        ether_type => {
            debug!("Unrecognized ether type {ether_type}");
            0
        }
    };
    Some((ether_type, proto_id))
}

/// Handle L2 frame.
#[inline]
#[allow(unsafe_code)]
fn handle_ether(
    mut m: Mbuf,
    tbl: &mut IpFragmentTable,
    dr: &mut IpFragDeathRow,
) -> Option<(i32, RecvResult)> {
    // l3 protocol, l4 protocol
    if let Some((ether_type, proto_id)) = parse_ether(&m) {
        m.adj(ETHER_HDR_LEN as _).ok()?;
        match ether_type {
            RTE_ETHER_TYPE_IPV4 => {
                let ptr = m.data_slice_mut().as_mut_ptr();
                // SAFETY: ffi
                let m = if unsafe { rte_ipv4_frag_pkt_is_fragmented(ptr.cast()) } == 0 {
                    Some(m)
                } else {
                    debug!("Packet need fragmentation");
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
                        #[allow(clippy::mem_forget)] // later dropped by head
                        mem::forget(m);
                        None // in need of more fragments
                    } else if mo != m.as_ptr() {
                        #[allow(clippy::mem_forget)] // later dropped by head
                        mem::forget(m);
                        #[allow(clippy::shadow_unrelated)] // they're related
                        let m = Mbuf::new_with_ptr(mo).ok()?;
                        Some(m)
                    } else {
                        Some(m)
                    }
                }?;
                return if proto_id == IP_NEXT_PROTO_UDP {
                    handle_ipv4_udp(m)
                } else {
                    debug!("Unrecognized proto id {proto_id}");
                    None
                };
            }
            RTE_ETHER_TYPE_IPV6 | RTE_ETHER_TYPE_ARP => {}
            ether_type => error!("Unsupported ether type {ether_type:x}"),
        }
    }
    None
}

#[allow(unsafe_code)]
impl RxAgent {
    /// Start an `RxAgent`, spawn a thread to do the polling job.
    pub(crate) fn start(socket_id: i32) -> Arc<Self> {
        let running = AtomicBool::new(true);
        let this = Arc::new(RxAgent {
            running,
            tasks: Mutex::new(BTreeSet::new()),
        });
        let that = Arc::clone(&this);
        let _handle = task::spawn_blocking(move || {
            let mut frag_tbl = IpFragmentTable::new(socket_id)?;
            let mut death_row = IpFragDeathRow::new(socket_id)?;
            // TODO error handling
            while that.running.load(Ordering::Acquire) {
                let tasks = that.tasks.lock().map_err(Error::from)?;
                let task_iter = tasks.iter();
                for &(port_id, queue_id) in task_iter {
                    let mut ptrs = vec![ptr::null_mut(); MAX_PKT_BURST as usize];
                    // SAFETY: ffi
                    let n = unsafe {
                        rte_eth_rx_burst(port_id, queue_id, ptrs.as_mut_ptr(), MAX_PKT_BURST)
                    };
                    trace!("{n} packets received");
                    for ptr in ptrs.into_iter().take(n as _) {
                        let m = Mbuf::new_with_ptr(ptr)?;
                        if let Some((sockfd, res)) = handle_ether(m, &mut frag_tbl, &mut death_row)
                        {
                            let _res = socket::put_mailbox(sockfd, res);
                        }
                    }
                }
            }
            info!("RxAgent thread terminated");
            Result::<()>::Ok(())
        });
        this
    }

    /// Stop the `RxAgent`.
    pub(crate) fn stop(self: &Arc<Self>) {
        self.running.store(false, Ordering::Release);
    }

    /// Register a (`port_id`, `queue_id`) to an `RxAgent`.
    pub(crate) fn register(self: &Arc<Self>, port_id: u16, queue_id: u16) -> Result<()> {
        let _ = self
            .tasks
            .lock()
            .map_err(Error::from)?
            .insert((port_id, queue_id));
        Ok(())
    }

    /// Unregister a (`port_id`, `queue_id`) from an `RxAgent`.
    pub(crate) fn unregister(self: &Arc<Self>, port_id: u16, queue_id: u16) -> Result<()> {
        let _ = self
            .tasks
            .lock()
            .map_err(Error::from)?
            .remove(&(port_id, queue_id));
        Ok(())
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
    pub(crate) fn register(
        self: &Arc<Self>,
        port_id: u16,
        queue_id: u16,
    ) -> Result<mpsc::Sender<Mbuf>> {
        let (tx, mut rx) = mpsc::channel::<Mbuf>(TX_CHAN_SIZE);
        let handle = self.rt.as_ref().ok_or(Error::NotStart)?.spawn(async move {
            let mut txbuf = TxBuffer::new(port_id, queue_id);
            while let Some(m) = rx.recv().await {
                let _res = txbuf.buffer(m); // TODO buffer could be full, should notify the caller.
            }
        });
        let _prev = self
            .tasks
            .lock()
            .map_err(Error::from)?
            .insert((port_id, queue_id), handle); // XXX
        Ok(tx)
    }

    /// Unregister a (`port_id`, `queue_id`) from a `TxAgent`.
    pub(crate) fn unregister(self: &Arc<Self>, port_id: u16, queue_id: u16) -> Result<()> {
        if let Some(handle) = self
            .tasks
            .lock()
            .map_err(Error::from)?
            .remove(&(port_id, queue_id))
        {
            handle.abort();
        }
        Ok(())
    }
}

impl Drop for TxAgent {
    fn drop(&mut self) {
        #[allow(clippy::unwrap_used)] // used in drop
        let rt = self.rt.take().unwrap();
        #[allow(clippy::mem_forget)] // not allowed to destroy a `Runtime` inside another
        mem::forget(rt);
    }
}

/// `TxBuffer` holding unsent mbufs.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
struct TxBuffer {
    /// Start index of mbufs.
    start: u16,
    /// End index of mbufs.
    end: u16,
    /// Total number of mbufs held.
    size: u16,
    /// `port_id` that the mbufs are sent to.
    port_id: u16,
    /// `queue_id` that the mbufs are sent to.
    queue_id: u16,
    /// `mbuf`s held.
    mbufs: [*mut rte_mbuf; TX_BUF_SIZE as usize],
}

// SAFETY: `TxBuffer` is `Send`.
#[allow(unsafe_code)]
unsafe impl Send for TxBuffer {}

#[allow(unsafe_code)]
impl TxBuffer {
    /// Allocate a `TxBuffer` on the given port and queue.
    fn new(port_id: u16, queue_id: u16) -> Self {
        let mbufs = [ptr::null_mut(); TX_BUF_SIZE as usize];
        Self {
            start: 0,
            end: 0,
            size: 0,
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
                let ether_dst = rte_pktmbuf_prepend(m, ETHER_HDR_LEN).cast::<rte_ether_hdr>();
                (*ether_dst).ether_type = ether_src.ether_type;
                rte_ether_addr_copy(&(*ether_src).src_addr, &mut (*ether_dst).src_addr);
                rte_ether_addr_copy(&(*ether_src).dst_addr, &mut (*ether_dst).dst_addr);
            }
        }
    }

    /// Send any packets queued up for transmission on a port and HW queue.
    #[inline]
    #[allow(clippy::too_many_lines)]
    fn buffer(&mut self, m: Mbuf) -> Result<()> {
        // Put the new mbuf at the end of buffer.
        if m.pkt_len() < RTE_ETHER_MTU as usize {
            if TX_BUF_SIZE.wrapping_sub(self.size) < 1 {
                return Err(Error::NoBuf);
            }
            #[allow(clippy::indexing_slicing)] // self.end < TX_BUF_SIZE
            self.mbufs[self.end as usize] = m.as_ptr();
            self.end = self.end.wrapping_add(1).wrapping_rem(TX_BUF_SIZE);
            self.size = self.size.wrapping_add(1);
            #[allow(clippy::mem_forget)] // later dropped by eth_tx_burst
            mem::forget(m);
        } else {
            // need fragment
            let nb_frags = m.pkt_len().wrapping_div(RTE_ETHER_MTU as _).wrapping_add(1);
            // Ensure there's enough buffer to hold fragmented data.
            if (TX_BUF_SIZE.wrapping_sub(self.size) as usize) < nb_frags.wrapping_add(1) {
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
            let _ = unsafe { rte_pktmbuf_adj(pm, ETHER_HDR_LEN) };
            // SAFETY: ffi
            let errno = unsafe {
                let l3_type = (*pm).packet_type_union.packet_type & RTE_PTYPE_L3_MASK;
                if l3_type == RTE_PTYPE_L3_IPV4 {
                    #[allow(clippy::cast_possible_truncation)]
                    // nb_frags < TX_BUF_SIZE checked, 1500 < u16::MAX
                    rte_ipv4_fragment_packet(
                        pm,
                        frags.as_mut_ptr(),
                        nb_frags as _,
                        RTE_ETHER_MTU as _,
                        (*pm).pool,
                        (*pm).pool,
                    )
                } else if l3_type == RTE_PTYPE_L3_IPV6 {
                    #[allow(clippy::cast_possible_truncation)]
                    // nb_frags < TX_BUF_SIZE checked, 1500 < u16::MAX
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
            #[allow(
                clippy::shadow_unrelated,
                clippy::cast_sign_loss,
                clippy::cast_possible_truncation
            )]
            // they're related actually, errno is greater than 0, errno < TX_BUF_SIZE
            let nb_frags = errno as u16;
            Self::populate_ether_hdr(
                ether_src,
                frags.get(..nb_frags as _).ok_or(Error::OutOfRange)?,
            );
            for (i, mb) in frags.iter().enumerate() {
                #[allow(clippy::cast_possible_truncation)]
                // i < frags.len() < TX_BUF_SIZE < u16::MAX
                let idest = self.end.wrapping_add(i as _).wrapping_rem(TX_BUF_SIZE);
                #[allow(clippy::indexing_slicing)] // idest < TX_BUF_SIZE
                self.mbufs[idest as usize] = *mb;
            }
            self.end = self.end.wrapping_add(nb_frags).wrapping_rem(TX_BUF_SIZE);
            self.size = self.size.wrapping_add(nb_frags);
        }

        if self.start < self.end {
            let to_send = self.end.wrapping_sub(self.start);
            // if to_send < TX_BUF_THRESH {
            //     return;
            // }
            // SAFETY: ffi
            let sent = unsafe {
                rte_eth_tx_burst(
                    self.port_id,
                    self.queue_id,
                    self.mbufs.as_mut_ptr().add(self.start as _),
                    to_send,
                )
            };
            trace!("{sent} packets sent");
            self.start = self.start.wrapping_add(sent);
        } else {
            #[allow(clippy::cast_possible_truncation)] // 2 * TX_BUF_SIZE < u16::MAX
            let to_send1 = TX_BUF_SIZE.wrapping_sub(self.start);
            #[allow(clippy::cast_possible_truncation)] // self.end < TX_BUF_SIZE < u16::MAX
            let to_send2 = self.end;
            // if to_send1 + to_send2 < TX_BUF_THRESH {
            //     return;
            // }
            // SAFETY: ffi
            let mut sent = unsafe {
                rte_eth_tx_burst(
                    self.port_id,
                    self.queue_id,
                    self.mbufs.as_mut_ptr().add(self.start as _),
                    to_send1,
                )
            };
            // SAFETY: ffi
            let sent2 = unsafe {
                rte_eth_tx_burst(
                    self.port_id,
                    self.queue_id,
                    self.mbufs.as_mut_ptr(),
                    to_send2,
                )
            };
            sent = sent.wrapping_add(sent2);
            trace!("{sent} packets sent");
            self.start = self.start.wrapping_add(sent);
            self.start = self.start.wrapping_rem(TX_BUF_SIZE);
        }
        Ok(())
    }
}
