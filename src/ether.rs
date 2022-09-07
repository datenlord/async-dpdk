//! General expression of packets.

use dpdk_sys::{rte_ether_addr, rte_ether_addr_copy, rte_ether_hdr};

use crate::{
    alloc,
    eth_dev::{EthDev, EthRxQueue, EthTxQueue},
    mbuf::Mbuf,
    mempool::Mempool,
    protocol::{Packet, Protocol},
    Result,
};
use std::{mem, sync::Arc};

/// Ethernet packet.
#[derive(Debug)]
pub struct EthPacket<'a> {
    #[allow(dead_code)]
    frags: Vec<&'a mut [u8]>,
}

#[allow(unsafe_code)]
impl EthPacket<'_> {
    /// Create a Ethernet packet.
    pub fn new(src: rte_ether_addr, dst: rte_ether_addr, protocol: u16) -> Self {
        let data = alloc::slice_mut(mem::size_of::<rte_ether_hdr>());
        let hdr = data.as_mut_ptr() as *mut rte_ether_hdr;
        unsafe {
            rte_ether_addr_copy(&src, &mut (*hdr).src_addr);
            rte_ether_addr_copy(&dst, &mut (*hdr).dst_addr);
            (*hdr).ether_type = protocol.to_be();
        }
        let frags = vec![data];
        Self { frags }
    }
}

impl Packet for EthPacket<'_> {
    fn from_mbuf(m: Mbuf) -> Self {
        let mut frags = vec![];
        let mut cur = &m;
        loop {
            let data = cur.data_slice();
            let buf = alloc::slice_mut(data.len());
            buf.copy_from_slice(data);
            frags.push(buf);
            if let Some(c) = cur.next() {
                cur = c;
            } else {
                break;
            }
        }
        EthPacket { frags }
    }

    fn into_mbuf(self, mp: &Mempool) -> Result<Mbuf> {
        let mut tail = Mbuf::new(&mp)?;
        let mut head: Option<Mbuf> = None;
        for frag in self.frags.into_iter() {
            let mut frag = frag;
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
                frag = &mut frag[delta..];
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
    rx: EthRxQueue,
    tx: EthTxQueue,
}

impl Protocol for Ethernet {
    type Pkt = EthPacket<'static>; // XXX wtf

    fn bind(port_id: u16) -> Result<Self> {
        let mut dev = EthDev::new(port_id, 1, 1)?;
        let rx = EthRxQueue::init(&mut dev, 0)?;
        let tx = EthTxQueue::init(&mut dev, 0)?;
        Ok(Self { dev, rx, tx })
    }
}

impl Ethernet {
    /// Send a L2 packet.
    pub async fn send(&self, pkt: EthPacket<'_>) -> Result<()> {
        self.tx.send(pkt).await
    }

    /// Recv a L2 packet.
    pub async fn recv(&mut self) -> Result<EthPacket<'_>> {
        self.rx.recv().await
    }
}
