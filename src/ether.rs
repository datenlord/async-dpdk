//! General expression of packets.

use crate::{
    eal::Eal,
    eth_dev::{EthDev, EthRxQueue, EthTxQueue},
    mbuf::Mbuf,
    mempool::Mempool,
    protocol::{Packet, Protocol},
    Result,
};
use bytes::{BufMut, BytesMut};
use dpdk_sys::{rte_ether_addr, rte_ether_addr_copy, rte_ether_hdr};
use std::{mem, sync::Arc};

#[allow(unused_macros)]
macro_rules! is_bad_iova {
    ($virt:expr) => {
        // #[allow(unsafe_code)]
        unsafe { dpdk_sys::rte_mem_virt2iova($virt.cast()) == u64::MAX }
    };
}

/// Ethernet packet.
#[derive(Debug)]
pub struct EthPacket {
    #[allow(dead_code)]
    frags: Vec<BytesMut>,
}

#[allow(unsafe_code)]
impl EthPacket {
    /// Create a Ethernet packet.
    pub fn new(src: rte_ether_addr, dst: rte_ether_addr, protocol: u16) -> Self {
        // let data = alloc::slice_mut(mem::size_of::<rte_ether_hdr>()); // XXX: mem leak
        let mut data = BytesMut::with_capacity(mem::size_of::<rte_ether_hdr>());
        let hdr = data.as_mut_ptr() as *mut rte_ether_hdr;
        unsafe {
            rte_ether_addr_copy(&src, &mut (*hdr).src_addr);
            rte_ether_addr_copy(&dst, &mut (*hdr).dst_addr);
            (*hdr).ether_type = protocol.to_be();
        };
        let frags = vec![data];
        Self { frags }
    }
}

#[allow(unsafe_code)]
impl Packet for EthPacket {
    fn from_mbuf(m: Mbuf) -> Self {
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
        EthPacket { frags }
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
        Ok(head.unwrap_or(tail))
    }
}

/// Ethernet protocol implementation
#[derive(Debug)]
pub struct Ethernet {
    #[allow(dead_code)]
    dev: Arc<EthDev>,
    rx: Arc<EthRxQueue>,
    tx: Arc<EthTxQueue>,
}

impl Protocol for Ethernet {
    type Pkt = EthPacket;

    fn bind(ctx: &Arc<Eal>, port_id: u16) -> Result<Self> {
        let mut dev = EthDev::new(ctx, port_id, 1, 1)?;
        let rx = EthRxQueue::init(&mut dev, 0)?;
        let tx = EthTxQueue::init(&mut dev, 0)?;
        Ok(Self { dev, rx, tx })
    }
}

impl Ethernet {
    /// Send a L2 packet.
    pub async fn send(&self, pkt: EthPacket) -> Result<()> {
        self.tx.send(pkt).await
    }

    /// Recv a L2 packet.
    pub async fn recv(&mut self) -> Result<EthPacket> {
        self.rx.recv().await
    }
}
