//! Generic L3 packet.

use crate::{
    mbuf::Mbuf,
    mempool::PktMempool,
    proto::{L3Protocol, L4Protocol, Protocol, ETHER_HDR_LEN, PTYPE_L2_ETHER},
    Result,
};
use bytes::{BufMut, BytesMut};
use dpdk_sys::{RTE_PTYPE_L3_MASK, RTE_PTYPE_L4_MASK};

/// Mask for L3 protocol id in `rte_mbuf`.
const L3_MASK: u32 = RTE_PTYPE_L3_MASK;

/// Mask for L4 protocol id in `rte_mbuf`.
const L4_MASK: u32 = RTE_PTYPE_L4_MASK;

/// Generic packet. By default, it's an network layer packet.
///
/// It is equivalent to a `Mbuf` without L2 header. It consists of several memory slices for easy
/// L3/L4 protocol headers constructing and parsing.
#[derive(Debug)]
pub struct Packet {
    /// L3 (Network layer) protocol.
    pub l3protocol: L3Protocol,
    /// L4 (Transport layer) protocol.
    pub l4protocol: L4Protocol,
    /// Fragments of slices. `BytesMut` indicates that `Packet` owns its fragments exclusively.
    pub(crate) frags: Vec<BytesMut>,
}

#[allow(unsafe_code)]
impl Packet {
    /// Get a new generic Packet instance.
    #[inline]
    #[must_use]
    pub fn new(l3protocol: L3Protocol, l4protocol: L4Protocol) -> Self {
        Self {
            frags: vec![],
            l3protocol,
            l4protocol,
        }
    }

    /// Append fragment
    #[inline]
    pub fn append(&mut self, frag: BytesMut) {
        self.frags.push(frag);
    }

    /// Takes the ownership of a `Mbuf` and convert it to a `Packet` instance.
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn from_mbuf(m: Mbuf) -> Self {
        // XXX protocol information in rte_mbuf may be incorrect
        let (l3protocol, l4protocol): (L3Protocol, L4Protocol) = {
            // SAFETY: mbuf pointer checked upon its allocation
            let m = unsafe { &*m.as_ptr() };
            // SAFETY: access union type
            let pkt_type = unsafe { m.packet_type_union.packet_type };
            ((pkt_type & L3_MASK).into(), (pkt_type & L4_MASK).into())
        };
        let mut frags = vec![];
        let mut cur = m;

        let data = cur.data_slice();
        frags.push(data.into()); // TODO zero-copy

        while let Some(c) = cur.next() {
            #[allow(clippy::shadow_unrelated)] // related actually
            let data = c.data_slice();
            frags.push(data.into()); // TODO zero-copy
            cur = c;
        }
        Packet {
            l3protocol,
            l4protocol,
            frags,
        }
    }

    /// Convert a `Packet` to a `Mbuf`.
    #[allow(dead_code)]
    #[inline]
    pub(crate) fn into_mbuf(mut self, mp: &PktMempool) -> Result<Mbuf> {
        let mut tail = Mbuf::new(mp)?;
        let mut head: Option<Mbuf> = None;
        for frag in &mut self.frags {
            let mut len = frag.len();
            while len > tail.tailroom() {
                if tail.tailroom() == 0 {
                    if let Some(m) = head.as_mut() {
                        if let Err((err, _)) = m.chain(tail) {
                            return Err(err);
                        }
                    } else {
                        head = Some(tail);
                    }
                    // Out of space, should alloc a new mbuf.
                    tail = Mbuf::new(mp)?;
                }
                let delta = tail.tailroom();
                let data = tail.append(delta)?;
                #[allow(clippy::indexing_slicing)]
                // frag.len() > delta, implied by while condition
                data.copy_from_slice(&frag[..delta]); // TODO: zero-copy
                len = len.wrapping_sub(delta);
                // SAFETY: delta > frag's remain size
                unsafe {
                    frag.advance_mut(delta);
                }
            }
            let data = tail.append(len)?;
            data.copy_from_slice(frag); // TODO: zero-copy
        }
        let mbuf = head.unwrap_or(tail);
        // SAFETY: mbuf pointer checked upon its allocation
        let m = unsafe { &mut *(mbuf.as_ptr()) };
        m.packet_type_union.packet_type =
            PTYPE_L2_ETHER | self.l3protocol as u32 | self.l4protocol as u32;
        // SAFETY: access to union field
        unsafe {
            m.tx_offload_union
                .tx_offload_struct
                .set_l2_len(ETHER_HDR_LEN);
            m.tx_offload_union
                .tx_offload_struct
                .set_l3_len(self.l3protocol.length());
            m.tx_offload_union
                .tx_offload_struct
                .set_l4_len(self.l4protocol.length());
        }
        Ok(mbuf)
    }
}
