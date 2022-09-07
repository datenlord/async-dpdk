//! Protocol trait

// use crate::{Result, Error};
use crate::{mbuf::Mbuf, mempool::Mempool, Result};

/// Packet is a general trait for l2/l3/l4 protocol packets.
/// It can be converted from and into Mbuf.
/// The conversion should be zero-copy!!!
pub trait Packet {
    /// Generate Packet from a Mbuf
    fn from_mbuf(m: Mbuf) -> Self;
    /// Convert Packet into a Mbuf.
    fn into_mbuf(self, mp: &Mempool) -> Result<Mbuf>;
}

/// Protocol trait
pub trait Protocol: Sized {
    /// The specific packet for the protocol
    type Pkt: Packet;

    /// Binding protocol to a specific device.
    fn bind(port_id: u16) -> Result<Self>;
}
