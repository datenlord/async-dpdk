//! RX/TX agent thread, which polls queues in background.

use crate::mbuf::Mbuf;
use crate::proto::{L3Protocol, L4Protocol, Protocol, ETHER_HDR_LEN, IP_NEXT_PROTO_UDP};
use crate::socket::{self, RecvResult};
use crate::udp::handle_ipv4_udp;
use crate::{Error, Result};
use dpdk_sys::{
    rte_eth_rx_burst, rte_eth_tx_burst, rte_ether_addr_copy, rte_ether_hdr, rte_free,
    rte_get_tsc_hz, rte_ip_frag_death_row, rte_ip_frag_table_create, rte_ip_frag_table_destroy,
    rte_ip_frag_tbl, rte_ipv4_frag_pkt_is_fragmented, rte_ipv4_frag_reassemble_packet,
    rte_ipv4_fragment_packet, rte_ipv4_hdr, rte_ipv6_fragment_packet, rte_ipv6_hdr, rte_mbuf,
    rte_mbuf_buf_addr, rte_pktmbuf_adj, rte_pktmbuf_prepend, rte_rdtsc, rte_zmalloc_socket,
    RTE_ETHER_MTU, RTE_ETHER_TYPE_ARP, RTE_ETHER_TYPE_IPV4, RTE_ETHER_TYPE_IPV6, RTE_PTYPE_L3_IPV4,
    RTE_PTYPE_L3_IPV6, RTE_PTYPE_L3_MASK,
};
use log::{debug, error, info, trace, warn};
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet, VecDeque};
use std::ffi::CString;
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
const TX_BUF_SIZE: usize = 1024;

/// Number of buckets in the hash table.
const IP_FRAG_TABLE_BUCKET_NUM: u32 = 128;

/// Number of entries per bucket (e.g. hash associativity). Should be power of two.
const IP_FRAG_TABLE_BUCKET_SIZE: u32 = 16;

/// Maximum number of entries that could be stored in the table. The value should be less
/// or equal then `bucket_num` * `bucket_entries`.
const IP_FRAG_TABLE_MAX_ENTRIES: u32 = 2048;

/// An agent thread continuously receives.
pub(crate) struct RxAgent {
    /// Whether the thread is running.
    running: AtomicBool,
    /// A set of queues to be polled.
    tasks: Mutex<BTreeSet<(u16, u16)>>,
}

/// An agent thread doing sending.
pub(crate) struct TxAgent {
    /// Single-threaded `Runtime`.
    rt: Option<Runtime>,
    /// For each queue registered, there's a Task polling it.
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
        // SAFETY: ffi
        let max_cycles = unsafe { rte_get_tsc_hz() }; // 1s

        // SAFETY: pointer checked later
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
        // SAFETY: tbl pointer checked in initialization
        unsafe { rte_ip_frag_table_destroy(self.as_mut_ptr()) };
    }
}

#[allow(unsafe_code)]
impl IpFragDeathRow {
    /// Create a new `IpFragDeathRow`.
    fn new(socket_id: i32) -> Result<Self> {
        let name = CString::new("death_row").map_err(Error::from)?;
        // SAFETY: pointer checked later
        let ptr = unsafe {
            rte_zmalloc_socket(
                name.as_ptr(),
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
        // SAFETY: pointer validity check in `IpFragDeathRow::new`
        unsafe {
            rte_free(self.as_mut_ptr().cast());
        }
    }
}

/// Check Ethernet packet validity and return its L3 protocol and L4 protocol.
/// This function returns `None` if it's invalid.
#[inline]
#[allow(unsafe_code)]
fn parse_ether_proto(m: &mut Mbuf) -> Option<(u32, u8)> {
    // SAFETY: *rte_mbuf checked
    let raw_mbuf = unsafe { &mut (*m.as_ptr()) };
    let data = m.data_slice();
    let remain = data.len().checked_sub(ETHER_HDR_LEN as _)?;
    // SAFETY: allowed in DPDK
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
    // SAFETY: set bitfields
    if proto_id == IP_NEXT_PROTO_UDP {
        unsafe {
            raw_mbuf
                .tx_offload_union
                .tx_offload_struct
                .set_l4_len(L4Protocol::Udp.length());
        }
    };
    Some((ether_type, proto_id))
}

/// Handle L2 frame and parse the Ethernet header.
///
/// The protocols of Network and Transport Layer (L3 & L4) will be resolved, and the
/// packet will be dispatched to the corresponding L3 & L4 handling function.
#[inline]
#[allow(unsafe_code)]
fn handle_ether(
    mut m: Mbuf,
    tbl: &mut IpFragmentTable,
    dr: &mut IpFragDeathRow,
) -> Option<(i32, RecvResult)> {
    // l3 protocol, l4 protocol
    if let Some((ether_type, proto_id)) = parse_ether_proto(&mut m) {
        m.adj(ETHER_HDR_LEN as _).ok()?;
        match ether_type {
            RTE_ETHER_TYPE_IPV4 => {
                let ip_hdr = m.data_slice_mut().as_mut_ptr();
                // SAFETY: *rte_mbuf checked
                let m = if unsafe { rte_ipv4_frag_pkt_is_fragmented(ip_hdr.cast()) } == 0 {
                    Some(m)
                } else {
                    log::debug!("Packet need fragmentation");
                    // SAFETY: pointers checked
                    let mo = unsafe {
                        rte_ipv4_frag_reassemble_packet(
                            tbl.as_mut_ptr(),
                            dr.as_mut_ptr(),
                            m.as_ptr(),
                            rte_rdtsc(),
                            ip_hdr.cast(),
                        )
                    };
                    if mo.is_null() {
                        #[allow(clippy::mem_forget)] // later dropped by head
                        mem::forget(m);
                        None // in need of more fragments
                    } else if mo != m.as_ptr() {
                        #[allow(clippy::mem_forget)] // later dropped by head
                        mem::forget(m);
                        let new_m = Mbuf::new_with_ptr(mo).ok()?;
                        Some(new_m) // fragmented ip packet
                    } else {
                        Some(m) // unfragmented ip packet
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
            while that.running.load(Ordering::Acquire) {
                let tasks = that.tasks.lock().map_err(Error::from)?;
                let task_iter = tasks.iter();
                for &(port_id, queue_id) in task_iter {
                    let mut ptrs = vec![ptr::null_mut(); MAX_PKT_BURST as usize];
                    // SAFETY: `n` packets at the front are valid
                    let n = unsafe {
                        rte_eth_rx_burst(port_id, queue_id, ptrs.as_mut_ptr(), MAX_PKT_BURST)
                    };
                    trace!("{n} packets received");
                    for ptr in ptrs.into_iter().take(n as _) {
                        let m = Mbuf::new_with_ptr(ptr)?;
                        if let Some((sockfd, res)) = handle_ether(m, &mut frag_tbl, &mut death_row)
                        {
                            let res = socket::put_mailbox(sockfd, res);
                            if let Err(e) = res {
                                error!("An error {e} occurred in `put_mailbox`");
                            }
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
    ///
    /// Adds a (`port_id`, `queue_id`) pair to the set to be polled.
    ///
    /// # Errors
    ///
    /// - Returns an `Error::NotStart` if the agent had already been stopped.
    /// - Returns an `Error::Already` if the pair had already been registered.
    pub(crate) fn register(self: &Arc<Self>, port_id: u16, queue_id: u16) -> Result<()> {
        if !self.running.load(Ordering::Acquire) {
            return Err(Error::NotStart);
        }
        if !self
            .tasks
            .lock()
            .map_err(Error::from)?
            .insert((port_id, queue_id))
        {
            return Err(Error::Already);
        }
        Ok(())
    }

    /// Unregister a (`port_id`, `queue_id`) from an `RxAgent`.
    ///
    /// Removes the (`port_id`, `queue_id`) pair from the polled set.
    ///
    /// # Errors
    ///
    /// - Returns an `Error::NotStart` if the agent had already been stopped.
    /// - Returns an `Error::NotExist` if the pair had not been registered.
    pub(crate) fn unregister(self: &Arc<Self>, port_id: u16, queue_id: u16) -> Result<()> {
        if !self.running.load(Ordering::Acquire) {
            return Err(Error::NotStart);
        }
        if !self
            .tasks
            .lock()
            .map_err(Error::from)?
            .remove(&(port_id, queue_id))
        {
            return Err(Error::NotExist);
        }
        Ok(())
    }
}

impl Drop for RxAgent {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

#[allow(unsafe_code)]
impl TxAgent {
    /// Start a `TxBuffer`, spawn a thread to do the sending job.
    pub(crate) fn start() -> Arc<Self> {
        #[allow(clippy::unwrap_used)] // impossible to panic since io and timer disabled
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
    ///
    /// It will spawn a new `Task` polling the given queue.
    ///
    /// # Errors
    ///
    /// - `Error::Already`: if the caller tries to register a queue that is
    /// already registered.
    pub(crate) fn register(
        self: &Arc<Self>,
        port_id: u16,
        queue_id: u16,
    ) -> Result<mpsc::Sender<Mbuf>> {
        let mut tasks = self.tasks.lock().map_err(Error::from)?;
        let entry = tasks.entry((port_id, queue_id));
        if matches!(entry, Entry::Occupied(_)) {
            return Err(Error::Already);
        }

        let (tx, mut rx) = mpsc::channel::<Mbuf>(TX_CHAN_SIZE);
        let handle = self.rt.as_ref().ok_or(Error::NotStart)?.spawn(async move {
            let mut txbuf = TxBuffer::new(port_id, queue_id);
            while let Some(m) = rx.recv().await {
                let res = txbuf.buffer(m);
                if let Err(e) = res {
                    // TODO buffer could be full, should notify the caller.
                    error!("An error {e} occurred in bufferring");
                }
            }
        });
        #[allow(clippy::let_underscore_future)] // spawned future polled in background
        let _ = entry.or_insert(handle);

        Ok(tx)
    }

    /// Unregister a (`port_id`, `queue_id`) from a `TxAgent`.
    ///
    /// It will aborts the specific `Task` doing polling.
    ///
    /// # Errors
    ///
    /// - `Error::NotExist`: if the caller tries to unregister a queue that is
    /// not registered.
    pub(crate) fn unregister(self: &Arc<Self>, port_id: u16, queue_id: u16) -> Result<()> {
        let handle = self
            .tasks
            .lock()
            .map_err(Error::from)?
            .remove(&(port_id, queue_id))
            .ok_or(Error::NotExist)?;
        handle.abort();
        Ok(())
    }
}

impl Drop for TxAgent {
    fn drop(&mut self) {
        // cancel tasks
        if let Ok(mut tasks) = self.tasks.lock() {
            for (_, handle) in tasks.iter() {
                handle.abort();
            }
            tasks.clear();
        }
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

/// `TxBuffer` holding unsent mbufs.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
struct TxBuffer {
    /// `port_id` that the mbufs are sent to.
    port_id: u16,
    /// `queue_id` that the mbufs are sent to.
    queue_id: u16,
    /// `mbuf`s held.
    mbufs: VecDeque<*mut rte_mbuf>,
}

// SAFETY: `TxBuffer` is globally accessed.
#[allow(unsafe_code)]
unsafe impl Send for TxBuffer {}

#[allow(unsafe_code)]
impl TxBuffer {
    /// Allocate a `TxBuffer` on the given port and queue.
    fn new(port_id: u16, queue_id: u16) -> Self {
        Self {
            port_id,
            queue_id,
            mbufs: VecDeque::with_capacity(TX_BUF_SIZE),
        }
    }

    /// Populate the fragmented IP packets.
    #[inline]
    fn populate_ether_hdr(ether_src: &rte_ether_hdr, mbufs: &[*mut rte_mbuf]) {
        for &m in mbufs {
            // SAFETY: DPDK fragmented packets are newly allocated with an unused headroom of 128 bytes,
            // which is larger than an Ethernet header. Thus the returned pointer is valid.
            unsafe {
                #[allow(clippy::cast_ptr_alignment)] // allowed in DPDK
                let ether_dst = rte_pktmbuf_prepend(m, ETHER_HDR_LEN).cast::<rte_ether_hdr>();
                (*ether_dst).ether_type = ether_src.ether_type;
                rte_ether_addr_copy(&ether_src.src_addr, &mut (*ether_dst).src_addr);
                rte_ether_addr_copy(&ether_src.dst_addr, &mut (*ether_dst).dst_addr);
            }
        }
    }

    /// Do IP fragmentation and buffer them.
    #[inline]
    fn do_fragment(&mut self, m: Mbuf) -> Result<()> {
        // need fragment
        let exp_nb_frags = m.pkt_len().wrapping_div(RTE_ETHER_MTU as _).wrapping_add(1);
        // Ensure there's enough buffer to hold fragmented data.
        if TX_BUF_SIZE.wrapping_sub(self.mbufs.len()) < exp_nb_frags.wrapping_add(1) {
            return Err(Error::NoBuf);
        }
        let mut frags: Vec<*mut rte_mbuf> = vec![ptr::null_mut(); exp_nb_frags];
        let pm = m.as_ptr();
        // SAFETY: pm checked in `Mbuf::new`
        #[allow(clippy::cast_ptr_alignment)]
        let ether_src = unsafe {
            &*(rte_mbuf_buf_addr(pm, (*pm).pool)
                .add((*pm).data_off as _)
                .cast::<rte_ether_hdr>())
        };
        // SAFETY: pm checked in m's initialization
        let _ = unsafe { rte_pktmbuf_adj(pm, ETHER_HDR_LEN) };
        // SAFETY: `return_val` packets at the front are valid
        let errno = unsafe {
            let l3_type = (*pm).packet_type_union.packet_type & RTE_PTYPE_L3_MASK;
            if l3_type == RTE_PTYPE_L3_IPV4 {
                #[allow(clippy::cast_possible_truncation)] // MTU = 1500 < u16::MAX
                rte_ipv4_fragment_packet(
                    pm,
                    frags.as_mut_ptr(),
                    exp_nb_frags.try_into().map_err(Error::from)?,
                    RTE_ETHER_MTU as _,
                    (*pm).pool,
                    (*pm).pool,
                )
            } else if l3_type == RTE_PTYPE_L3_IPV6 {
                #[allow(clippy::cast_possible_truncation)] // MTU = 1500 < u16::MAX
                rte_ipv6_fragment_packet(
                    pm,
                    frags.as_mut_ptr(),
                    exp_nb_frags.try_into().map_err(Error::from)?,
                    RTE_ETHER_MTU as _,
                    (*pm).pool,
                    (*pm).pool,
                )
            } else {
                -1
            }
        };
        Error::from_ret(errno)?;
        #[allow(clippy::cast_sign_loss)] // errno checked
        let nb_frags = errno as usize;
        log::trace!("tx: nb_frags={nb_frags}");

        Self::populate_ether_hdr(ether_src, frags.get(..nb_frags).ok_or(Error::OutOfRange)?);
        for mb in &frags {
            self.mbufs.push_back(*mb);
        }
        #[allow(clippy::mem_forget)] // later dropped by `eth_tx_burst`
        mem::forget(m);
        Ok(())
    }

    /// Send any packets queued up for transmission on a port and HW queue.
    #[inline]
    fn buffer(&mut self, m: Mbuf) -> Result<()> {
        // Put the new mbuf at the end of buffer.
        if m.pkt_len() < RTE_ETHER_MTU as usize {
            if TX_BUF_SIZE < self.mbufs.len() {
                return Err(Error::NoBuf);
            }
            self.mbufs.push_back(m.as_ptr());
            #[allow(clippy::mem_forget)] // later dropped by `eth_tx_burst`
            mem::forget(m);
        } else {
            // need fragmentation
            self.do_fragment(m)?;
        }

        let (msg1, msg2) = self.mbufs.as_mut_slices();
        let mut sent = 0_u16;
        let mut unsent = true;
        // XXX pop_front???
        if !msg1.is_empty() {
            #[allow(clippy::cast_possible_truncation)]
            // SAFETY: msg1 length checked
            let sent1 = unsafe {
                rte_eth_tx_burst(
                    self.port_id,
                    self.queue_id,
                    msg1.as_mut_ptr(),
                    msg1.len() as _,
                )
            };
            if sent1 as usize == msg1.len() {
                unsent = false;
            }
            sent = sent.wrapping_add(sent1);
        }
        if !unsent && !msg2.is_empty() {
            #[allow(clippy::cast_possible_truncation)]
            // SAFETY: msg2 length checked
            let sent2 = unsafe {
                rte_eth_tx_burst(
                    self.port_id,
                    self.queue_id,
                    msg2.as_mut_ptr(),
                    msg2.len() as _,
                )
            };
            sent = sent.wrapping_add(sent2);
        }

        for _ in 0..sent {
            let _ = self.mbufs.pop_front(); // sent messages
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{RxAgent, TxAgent};
    use crate::{test_utils, Error};

    #[tokio::test]
    async fn test_tx_agent() {
        test_utils::dpdk_setup();
        let tx_agent = TxAgent::start();
        let _ = tx_agent.register(0, 0).unwrap();
        assert!(matches!(
            tx_agent.register(0, 0).unwrap_err(),
            Error::Already
        ));
        tx_agent.unregister(0, 0).unwrap();
        assert!(matches!(
            tx_agent.unregister(0, 0).unwrap_err(),
            Error::NotExist
        ));
    }

    #[tokio::test]
    async fn test_rx_agent() {
        test_utils::dpdk_setup();
        let rx_agent = RxAgent::start(0);
        rx_agent.register(0, 0).unwrap();
        assert!(matches!(
            rx_agent.register(0, 0).unwrap_err(),
            Error::Already
        ));
        rx_agent.unregister(0, 0).unwrap();
        assert!(matches!(
            rx_agent.unregister(0, 0).unwrap_err(),
            Error::NotExist
        ));
        rx_agent.stop();
    }
}
