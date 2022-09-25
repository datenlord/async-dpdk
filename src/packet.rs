//! General expression of packets.

use crate::{
    mbuf::Mbuf,
    mempool::Mempool,
    protocol::{L2Protocol, L3Protocol, L4Protocol, Packet},
    Result,
};
use bytes::{BufMut, BytesMut};
use dpdk_sys::{RTE_PTYPE_L2_MASK, RTE_PTYPE_L3_MASK, RTE_PTYPE_L4_MASK};

#[allow(unused_macros)]
macro_rules! is_bad_iova {
    ($virt:expr) => {
        // #[allow(unsafe_code)]
        unsafe { dpdk_sys::rte_mem_virt2iova($virt.cast()) == u64::MAX }
    };
}

/// Ethernet packet.
#[derive(Debug)]
pub struct GenericPacket {
    pub(crate) l2protocol: L2Protocol,
    pub(crate) l3protocol: L3Protocol,
    pub(crate) l4protocol: L4Protocol,
    pub(crate) frags: Vec<BytesMut>,
}

#[allow(unsafe_code)]
impl GenericPacket {
    /// Get a new GenericPacket instance.
    pub(crate) fn new(
        l2protocol: L2Protocol,
        l3protocol: L3Protocol,
        l4protocol: L4Protocol,
    ) -> Self {
        Self {
            frags: vec![],
            l2protocol,
            l3protocol,
            l4protocol,
        }
    }

    /// Append fragment
    pub(crate) fn append(&mut self, frag: BytesMut) {
        self.frags.push(frag);
    }
}

const L2_MASK: u32 = RTE_PTYPE_L2_MASK;
const L3_MASK: u32 = RTE_PTYPE_L3_MASK;
const L4_MASK: u32 = RTE_PTYPE_L4_MASK;

#[allow(unsafe_code)]
impl Packet for GenericPacket {
    fn from_mbuf(m: Mbuf) -> Result<Self> {
        let (l2protocol, l3protocol, l4protocol) = {
            let m = unsafe { &*m.as_ptr() };
            let pkt_type = unsafe { m.packet_type_union.packet_type };
            (
                (pkt_type & L2_MASK).into(),
                (pkt_type & L3_MASK).into(),
                (pkt_type & L4_MASK).into(),
            )
        };
        let mut frags = vec![];
        let mut cur = &m;
        loop {
            let data = cur.data_slice();
            frags.push(data.into()); // TODO zero-copy
            if let Some(c) = cur.next() {
                cur = c;
            } else {
                break;
            }
        }
        Ok(GenericPacket {
            l2protocol,
            l3protocol,
            l4protocol,
            frags,
        })
    }

    fn into_mbuf(mut self, mp: &Mempool) -> Result<Mbuf> {
        let mut tail = Mbuf::new(&mp)?;
        let mut head: Option<Mbuf> = None;
        for frag in self.frags.iter_mut() {
            let mut len = frag.len();
            while len > tail.tailroom() {
                if tail.tailroom() == 0 {
                    if let Some(m) = head.as_mut() {
                        m.chain(tail)?;
                    } else {
                        head = Some(tail);
                    }
                    tail = Mbuf::new(&mp)?;
                }
                let delta = tail.tailroom();
                let data = tail.append(delta)?;
                data.copy_from_slice(&frag[..delta]); // TODO: zero-copy
                len -= delta;
                unsafe {
                    frag.advance_mut(delta);
                }
            }
            let data = tail.append(len)?;
            data.copy_from_slice(frag); // TODO: zero-copy
        }
        let mbuf = head.unwrap_or(tail);
        let m = unsafe { &mut *(mbuf.as_ptr()) };
        m.packet_type_union.packet_type =
            self.l2protocol as u32 | self.l3protocol as u32 | self.l4protocol as u32;
        // XXX fill l2_len, l3_len and l4_len
        Ok(mbuf)
    }
}
