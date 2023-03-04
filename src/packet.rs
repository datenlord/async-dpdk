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
    #[allow(dead_code, clippy::needless_pass_by_value)]
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
        for cur in m.iter() {
            let data = cur.data_slice();
            frags.push(data.into());
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
                        if let Err((err, _)) = m.chain_mbuf(tail) {
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

#[cfg(test)]
mod tests {
    use super::Packet;
    use crate::{
        mbuf::Mbuf,
        mempool::{Mempool, PktMempool},
        proto::{L3Protocol, L4Protocol},
        test_utils,
    };
    use bytes::BytesMut;

    #[test]
    fn test() {
        test_utils::dpdk_setup();
        let mut pkt = Packet::new(L3Protocol::Ipv4, L4Protocol::Tcp);
        pkt.append(BytesMut::new());
        assert_eq!(pkt.frags.len(), 1);

        let mp = PktMempool::create("pktmpool", 10).unwrap();
        let mut mb1 = Mbuf::new(&mp).unwrap();
        let data = mb1.append(5).unwrap();
        data.copy_from_slice(&[0, 1, 2, 3, 4]);

        // Test conversion between mbuf and packet.
        let pkt = Packet::from_mbuf(mb1);
        assert_eq!(pkt.frags.len(), 1);
        assert_eq!(&pkt.frags[0][..], &[0, 1, 2, 3, 4]);

        let mb2 = pkt.into_mbuf(&mp).unwrap();
        assert_eq!(mb2.num_segs(), 1);
        assert_eq!(mb2.data_slice(), &[0, 1, 2, 3, 4]);

        // Test conversion betweem chained mbuf and packet.
        let mut mbs = Mbuf::new_bulk(&mp, 3).unwrap();
        for (i, m) in mbs.iter_mut().enumerate() {
            let data = m.append(5).unwrap();
            let i = i as u8;
            data.copy_from_slice(&[i, i, i, i, i]);
        }
        let tail2 = mbs.pop().unwrap();
        let mut tail1 = mbs.pop().unwrap();
        tail1.chain_mbuf(tail2).unwrap();
        let mut mb3 = mbs.pop().unwrap();
        mb3.chain_mbuf(tail1).unwrap();

        let pkt = Packet::from_mbuf(mb3);
        assert_eq!(pkt.frags.len(), 3);
        assert_eq!(&pkt.frags[0][..], &[0, 0, 0, 0, 0]);
        assert_eq!(&pkt.frags[1][..], &[1, 1, 1, 1, 1]);
        assert_eq!(&pkt.frags[2][..], &[2, 2, 2, 2, 2]);

        let mb4 = pkt.into_mbuf(&mp).unwrap();
        assert_eq!(mb4.num_segs(), 1);
        assert_eq!(
            mb4.data_slice(),
            &[0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2]
        );
    }
}
