//! General expression of packets.

use crate::{
    mbuf::Mbuf,
    mempool::Mempool,
    protocol::{L3Protocol, L4Protocol, Protocol, ETHER_HDR_LEN, PTYPE_L2_ETHER},
    Result,
};
use bytes::{BufMut, BytesMut};
use dpdk_sys::{RTE_PTYPE_L3_MASK, RTE_PTYPE_L4_MASK};

#[allow(unused_macros)]
macro_rules! is_bad_iova {
    ($virt:expr) => {
        // #[allow(unsafe_code)]
        unsafe { dpdk_sys::rte_mem_virt2iova($virt.cast()) == u64::MAX }
    };
}

const L3_MASK: u32 = RTE_PTYPE_L3_MASK;
const L4_MASK: u32 = RTE_PTYPE_L4_MASK;

/// Generic Packet.
#[derive(Debug)]
pub struct Packet {
    pub(crate) l3protocol: L3Protocol,
    pub(crate) l4protocol: L4Protocol,
    pub(crate) frags: Vec<BytesMut>,
}

#[allow(unsafe_code)]
impl Packet {
    /// Get a new generic Packet instance.
    pub(crate) fn new(l3protocol: L3Protocol, l4protocol: L4Protocol) -> Self {
        Self {
            frags: vec![],
            l3protocol,
            l4protocol,
        }
    }

    /// Append fragment
    pub(crate) fn append(&mut self, frag: BytesMut) {
        self.frags.push(frag);
    }

    pub(crate) fn from_mbuf(m: Mbuf) -> Result<Self> {
        // XXX protocol information in rte_mbuf may be incorrect
        let (l3protocol, l4protocol): (L3Protocol, L4Protocol) = {
            let m = unsafe { &*m.as_ptr() };
            let pkt_type = unsafe { m.packet_type_union.packet_type };
            ((pkt_type & L3_MASK).into(), (pkt_type & L4_MASK).into())
        };
        let mut frags = vec![];
        let mut cur = m;

        let data = cur.data_slice();
        frags.push(data.into()); // TODO zero-copy

        while let Some(c) = cur.next() {
            let data = c.data_slice();
            frags.push(data.into()); // TODO zero-copy
            cur = c;
        }
        Ok(Packet {
            l3protocol,
            l4protocol,
            frags,
        })
    }

    pub(crate) fn into_mbuf(mut self, mp: &Mempool) -> Result<Mbuf> {
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
            PTYPE_L2_ETHER | self.l3protocol as u32 | self.l4protocol as u32;
        // SAFETY: access to union field
        unsafe {
            m.tx_offload_union
                .tx_offload_struct
                .set_l2_len(ETHER_HDR_LEN as _);
            m.tx_offload_union
                .tx_offload_struct
                .set_l3_len(self.l3protocol.length() as _);
            m.tx_offload_union
                .tx_offload_struct
                .set_l4_len(self.l4protocol.length() as _);
        }
        Ok(mbuf)
    }
}
