//! Protocol trait

use dpdk_sys::{
    RTE_PTYPE_L2_ETHER, RTE_PTYPE_L2_ETHER_ARP, RTE_PTYPE_L3_IPV4, RTE_PTYPE_L3_IPV6,
    RTE_PTYPE_L4_TCP, RTE_PTYPE_L4_UDP,
};

// use crate::{Result, Error};
use crate::{mbuf::Mbuf, mempool::Mempool, Result};

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
/// L2 protocol.
pub enum L2Protocol {
    /// Ethernet packet type.
    Ether = RTE_PTYPE_L2_ETHER,
    /// ARP (Address Resolution Protocol) packet type.
    ARP = RTE_PTYPE_L2_ETHER_ARP,
}

impl Into<L2Protocol> for u32 {
    fn into(self) -> L2Protocol {
        match self {
            RTE_PTYPE_L2_ETHER => L2Protocol::Ether,
            RTE_PTYPE_L2_ETHER_ARP => L2Protocol::ARP,
            _ => unimplemented!("unknown l2 protocol number {self}"),
        }
    }
}

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
/// L3 protocol.
pub enum L3Protocol {
    /// Ipv4 packet type.
    Ipv4 = RTE_PTYPE_L3_IPV4,
    /// Ipv6 packet type.
    Ipv6 = RTE_PTYPE_L3_IPV6,
}

impl Into<L3Protocol> for u32 {
    fn into(self) -> L3Protocol {
        match self {
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
    /// UDP packet type.
    UDP = RTE_PTYPE_L4_UDP,
    /// TCP packet type.
    TCP = RTE_PTYPE_L4_TCP,
}

impl Into<L4Protocol> for u32 {
    fn into(self) -> L4Protocol {
        match self {
            RTE_PTYPE_L4_UDP => L4Protocol::UDP,
            RTE_PTYPE_L4_TCP => L4Protocol::TCP,
            _ => unimplemented!("unknown l4 protocol number {self}"),
        }
    }
}

/// Packet is a general trait for l2/l3/l4 protocol packets.
/// It can be converted from and into Mbuf.
/// The conversion should be zero-copy!!!
pub trait Packet: Sized {
    /// Generate Packet from a Mbuf
    fn from_mbuf(m: Mbuf) -> Result<Self>;
    /// Convert Packet into a Mbuf.
    fn into_mbuf(self, mp: &Mempool) -> Result<Mbuf>;
}
