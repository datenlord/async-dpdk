//! Protocol trait

use dpdk_sys::{
    RTE_PTYPE_L2_ETHER, RTE_PTYPE_L3_IPV4, RTE_PTYPE_L3_IPV6, RTE_PTYPE_L4_TCP, RTE_PTYPE_L4_UDP,
    RTE_PTYPE_UNKNOWN,
};

pub(crate) trait Protocol {
    fn length(&self) -> usize;
}

// pub(crate) const IP_NEXT_PROTO_TCP: u8 = 0x06;
pub(crate) const IP_NEXT_PROTO_UDP: u8 = 0x11;

pub(crate) const ETHER_HDR_LEN: usize = 14;
pub(crate) const PTYPE_L2_ETHER: u32 = RTE_PTYPE_L2_ETHER;

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
/// L3 protocol.
pub enum L3Protocol {
    /// Unknown L3 protocol
    Unknown = RTE_PTYPE_UNKNOWN,
    /// Ipv4 packet type.
    Ipv4 = RTE_PTYPE_L3_IPV4,
    /// Ipv6 packet type.
    Ipv6 = RTE_PTYPE_L3_IPV6,
}

impl Protocol for L3Protocol {
    fn length(&self) -> usize {
        match *self {
            L3Protocol::Ipv4 => 20,
            L3Protocol::Ipv6 => 40,
            L3Protocol::Unknown => 0,
        }
    }
}

impl Into<L3Protocol> for u32 {
    fn into(self) -> L3Protocol {
        match self {
            RTE_PTYPE_UNKNOWN => L3Protocol::Unknown,
            RTE_PTYPE_L3_IPV4 => L3Protocol::Ipv4,
            RTE_PTYPE_L3_IPV6 => L3Protocol::Ipv6,
            _ => unimplemented!("unknown l3 protocol number {self}"),
        }
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
/// L4 protocol.
pub enum L4Protocol {
    /// Unknown L4 protocol
    Unknown = RTE_PTYPE_UNKNOWN,
    /// UDP packet type.
    UDP = RTE_PTYPE_L4_UDP,
    /// TCP packet type.
    TCP = RTE_PTYPE_L4_TCP,
}

impl Protocol for L4Protocol {
    fn length(&self) -> usize {
        match *self {
            L4Protocol::UDP => 8,
            L4Protocol::TCP => 20,
            L4Protocol::Unknown => 0,
        }
    }
}

impl Into<L4Protocol> for u32 {
    fn into(self) -> L4Protocol {
        match self {
            RTE_PTYPE_UNKNOWN => L4Protocol::Unknown,
            RTE_PTYPE_L4_UDP => L4Protocol::UDP,
            RTE_PTYPE_L4_TCP => L4Protocol::TCP,
            _ => unimplemented!("unknown l4 protocol number {self}"),
        }
    }
}

// /// Packet is a general trait for l2/l3/l4 protocol packets.
// /// It can be converted from and into Mbuf.
// /// The conversion should be zero-copy!!!
// pub trait Packet: Sized {
//     /// Generate Packet from a Mbuf
//     fn from_mbuf(m: Mbuf) -> Result<Self>;
//     /// Convert Packet into a Mbuf.
//     fn into_mbuf(self, mp: &Mempool) -> Result<Mbuf>;
// }
