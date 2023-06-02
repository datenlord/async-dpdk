//! Protocols supported in this lib.

pub mod socket;
pub mod udp;

use dpdk_sys::{
    RTE_PTYPE_L2_ETHER, RTE_PTYPE_L3_IPV4, RTE_PTYPE_L3_IPV6, RTE_PTYPE_L4_TCP, RTE_PTYPE_L4_UDP,
    RTE_PTYPE_UNKNOWN,
};

/// Indicating that the struct is a packet for some protocol.
pub(crate) trait Protocol {
    /// Protocol header length.
    fn length(&self) -> u16;
}

/// UDP `proto_id`, to be populated in IP header.
pub(crate) const IP_NEXT_PROTO_UDP: u8 = 0x11;

/// Ethernet header length.
pub(crate) const ETHER_HDR_LEN: u16 = 14;

/// Ethernet proto number, to be populated in `rte_mbuf`.
pub(crate) const PTYPE_L2_ETHER: u32 = RTE_PTYPE_L2_ETHER;

#[repr(u32)]
#[non_exhaustive]
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
    fn length(&self) -> u16 {
        match *self {
            L3Protocol::Ipv4 => 20,
            L3Protocol::Ipv6 => 40,
            L3Protocol::Unknown => 0,
        }
    }
}

impl From<u32> for L3Protocol {
    #[inline]
    fn from(num: u32) -> L3Protocol {
        match num {
            RTE_PTYPE_L3_IPV4 => L3Protocol::Ipv4,
            RTE_PTYPE_L3_IPV6 => L3Protocol::Ipv6,
            _ => L3Protocol::Unknown,
        }
    }
}

#[repr(u32)]
#[non_exhaustive]
#[derive(Debug, Clone, Copy)]
/// L4 protocol.
pub enum L4Protocol {
    /// Unknown L4 protocol
    Unknown = RTE_PTYPE_UNKNOWN,
    /// UDP packet type.
    Udp = RTE_PTYPE_L4_UDP,
    /// TCP packet type.
    Tcp = RTE_PTYPE_L4_TCP,
}

impl Protocol for L4Protocol {
    fn length(&self) -> u16 {
        match *self {
            L4Protocol::Udp => 8,
            L4Protocol::Tcp => 20,
            L4Protocol::Unknown => 0,
        }
    }
}

impl From<u32> for L4Protocol {
    #[inline]
    fn from(num: u32) -> L4Protocol {
        match num {
            RTE_PTYPE_L4_UDP => L4Protocol::Udp,
            RTE_PTYPE_L4_TCP => L4Protocol::Tcp,
            _ => L4Protocol::Unknown,
        }
    }
}
