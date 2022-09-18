//! Protocol trait

use dpdk_sys::{
    RTE_PTYPE_L2_ETHER, RTE_PTYPE_L2_ETHER_ARP, RTE_PTYPE_L2_ETHER_LLDP, RTE_PTYPE_L2_ETHER_NSH,
    RTE_PTYPE_L2_ETHER_TIMESYNC, RTE_PTYPE_L2_ETHER_VLAN, RTE_PTYPE_L3_IPV4, RTE_PTYPE_L3_IPV6,
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
    /// Ethernet packet for time sync.
    TimeSync = RTE_PTYPE_L2_ETHER_TIMESYNC,
    /// ARP (Address Resolution Protocol) packet type.
    ARP = RTE_PTYPE_L2_ETHER_ARP,
    /// LLDP (Link Layer Discovery Protocol) packet type.
    LLDP = RTE_PTYPE_L2_ETHER_LLDP,
    /// NSH (Network Service Header) packet type.
    NSH = RTE_PTYPE_L2_ETHER_NSH,
    /// VLAN packet type.
    VLAN = RTE_PTYPE_L2_ETHER_VLAN,
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

#[repr(u32)]
#[derive(Debug, Clone, Copy)]
/// L4 protocol.
pub enum L4Protocol {
    /// UDP packet type.
    UDP = RTE_PTYPE_L4_UDP,
    /// TCP packet type.
    TCP = RTE_PTYPE_L4_TCP,
}

/// Packet is a general trait for l2/l3/l4 protocol packets.
/// It can be converted from and into Mbuf.
/// The conversion should be zero-copy!!!
pub trait L2Packet: Sized {
    /// Generate Packet from a Mbuf
    fn from_mbuf(m: Mbuf) -> Result<Self>;
    /// Convert Packet into a Mbuf.
    fn into_mbuf(self, mp: &Mempool) -> Result<Mbuf>;
}
