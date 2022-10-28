//! RX/TX agent thread
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
use crate::protocol::{ETHER_HDR_LEN, IP_NEXT_PROTO_UDP};
use crate::udp::{handle_ipv4_udp, handle_ipv6_udp};
use crate::{Error, Result};

const MAX_PKT_BURST: usize = 32;
const TX_CHAN_SIZE: usize = 256;
const TX_BUF_SIZE: usize = 1024;
#[allow(dead_code)]
const TX_BUF_THRESH: usize = 8;

const IP_FRAG_TABLE_BUCKET_NUM: u32 = 128;
const IP_FRAG_TABLE_BUCKET_SIZE: u32 = 16;
const IP_FRAG_TABLE_MAX_ENTRIES: u32 = 2048;

pub(crate) struct RxAgent {
    running: AtomicBool,
    tasks: Mutex<BTreeSet<(u16, u16)>>,
}

pub(crate) struct TxAgent {
    rt: Option<Runtime>,
    tasks: Mutex<BTreeMap<(u16, u16), JoinHandle<()>>>,
}

struct IpFragmentTable {
    tbl: NonNull<rte_ip_frag_tbl>,
}

struct IpFragDeathRow {
    dr: NonNull<rte_ip_frag_death_row>,
}

#[allow(unsafe_code)]
impl IpFragmentTable {
    fn new(socket_id: u32) -> Result<Self> {
        const MS_PER_S: u64 = 1000;
        let max_cycles = unsafe { (rte_get_tsc_hz() + MS_PER_S - 1) / MS_PER_S * MS_PER_S };
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
        unsafe { rte_ip_frag_table_destroy(self.as_mut_ptr()) };
    }
}

#[allow(unsafe_code)]
impl IpFragDeathRow {
    fn new(socket_id: u32) -> Result<Self> {
        let ptr = unsafe {
            rte_zmalloc_socket(
                cstring!("death_row"),
                mem::size_of::<rte_ip_frag_death_row>(),
                0,
                socket_id as _,
            ) as *mut rte_ip_frag_death_row
        };
        let dr = NonNull::new(ptr).ok_or(Error::NoMem)?;
        Ok(Self { dr })
    }
    fn as_mut_ptr(&mut self) -> *mut rte_ip_frag_death_row {
        self.dr.as_ptr()
    }
}

#[inline]
#[allow(unsafe_code)]
fn handle_ether(mut m: Mbuf, tbl: &mut IpFragmentTable, dr: &mut IpFragDeathRow) {
    // l3 protocol, l4 protocol
    let (ether_type, proto_id) = {
        let data = m.data_slice();
        let ether_hdr = unsafe { &*(data.as_ptr() as *const rte_ether_hdr) };
        let ether_type = ether_hdr.ether_type.to_be() as u32;

        let proto_id = match ether_type {
            RTE_ETHER_TYPE_IPV4 => {
                unsafe {
                    let pm = &mut *(m.as_ptr());
                    pm.tx_offload_union
                        .tx_offload_struct
                        .set_l3_len(mem::size_of::<rte_ipv4_hdr>() as _);
                }
                #[allow(trivial_casts)]
                let ip_hdr = unsafe {
                    &*((ether_hdr as *const rte_ether_hdr).add(1) as *const rte_ipv4_hdr)
                };
                ip_hdr.next_proto_id
            }
            RTE_ETHER_TYPE_IPV6 => {
                unsafe {
                    let pm = &mut *(m.as_ptr());
                    pm.tx_offload_union
                        .tx_offload_struct
                        .set_l3_len(mem::size_of::<rte_ipv6_hdr>() as _);
                }
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

    m.adj(ETHER_HDR_LEN).unwrap();

    match ether_type {
        RTE_ETHER_TYPE_IPV4 => {
            let ptr = m.data_slice_mut().as_mut_ptr();
            let new_m = if unsafe { rte_ipv4_frag_pkt_is_fragmented(ptr.cast()) } != 0 {
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
                    let m = Mbuf::new_with_ptr(mo).unwrap();
                    Some(m)
                } else {
                    Some(m)
                }
            } else {
                Some(m)
            };
            if new_m.is_none() {
                return;
            }
            let m = new_m.unwrap();
            match proto_id {
                IP_NEXT_PROTO_UDP => handle_ipv4_udp(m),
                _ => {}
            }
        }
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
    pub(crate) fn start(socket_id: u32) -> Arc<Self> {
        let running = AtomicBool::new(true);
        let this = Arc::new(RxAgent {
            running,
            tasks: Mutex::new(BTreeSet::new()),
        });
        let that = this.clone();
        let _ = task::spawn_blocking(move || {
            let mut frag_tbl = IpFragmentTable::new(socket_id).unwrap();
            let mut death_row = IpFragDeathRow::new(socket_id).unwrap();
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
                        handle_ether(m, &mut frag_tbl, &mut death_row);
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
                txbuf.buffer(m).unwrap();
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
    capacity: usize,
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
            size: 0,
            capacity: TX_BUF_SIZE,
            port_id,
            queue_id,
            mbufs,
        }
    }

    #[inline]
    fn populate_ether_hdr(ether_src: &rte_ether_hdr, mbufs: &[*mut rte_mbuf]) {
        for &m in mbufs {
            unsafe {
                let ether_dst = rte_pktmbuf_prepend(m, ETHER_HDR_LEN as _) as *mut rte_ether_hdr;
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
            let ether_src = unsafe {
                &*(rte_mbuf_buf_addr(pm, (*pm).pool).add((*pm).data_off as _)
                    as *const rte_ether_hdr)
            };
            let _ = unsafe { rte_pktmbuf_adj(pm, ETHER_HDR_LEN as _) };
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
            let nb_frags = errno as usize;
            Self::populate_ether_hdr(ether_src, &frags[..nb_frags]);
            for (i, m) in frags.iter().enumerate() {
                let idest = (self.end + i) % self.capacity;
                self.mbufs[idest] = *m;
            }
            self.end = (self.end + nb_frags) % self.capacity;
            self.size += nb_frags;
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
            let to_send1 = self.capacity - self.start;
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
            self.start = self.start.wrapping_add(sent as _) % self.capacity;
        }
        Ok(())
    }
}
