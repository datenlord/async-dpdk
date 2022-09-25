//! UDP implementation

use crate::{
    eth_dev::EthTxQueue,
    net_dev,
    packet::GenericPacket,
    protocol::{L2Protocol, L3Protocol, L4Protocol},
    socket::{self, Mailbox},
    Error, Result,
};
use bytes::{Buf, BufMut, BytesMut};
use dpdk_sys::{rte_ether_hdr, rte_ipv4_hdr, rte_udp_hdr, RTE_ETHER_TYPE_IPV4};
use std::{
    mem,
    net::{SocketAddr, ToSocketAddrs},
    sync::{Arc, Mutex},
};

/// A UDP socket.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct UdpSocket {
    sockfd: i32,
    tx: Arc<EthTxQueue>,
    mailbox: Arc<Mutex<Mailbox>>,
}

impl UdpSocket {
    /// Creates a UDP socket from the given address.
    pub fn bind<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        while let Some(addr) = addr.to_socket_addrs().unwrap().next() {
            if let Ok(sockfd) = socket::bind_fd(addr) {
                if let Ok((_, tx)) = net_dev::find_dev_by_ip(addr.ip()) {
                    let mailbox = socket::alloc_mailbox(sockfd);
                    return Ok(UdpSocket {
                        sockfd,
                        tx,
                        mailbox,
                    });
                }
                socket::free_fd(sockfd).unwrap();
                return Err(Error::BadAddress);
            }
        }
        Err(Error::NoBuf)
    }

    /// Receives a single datagram message on the socket. On success, returns
    /// the number of bytes read and the origin.
    pub async fn recv_from(&mut self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        let rx = self.mailbox.lock().unwrap().recv();
        let (addr, data) = rx.await.unwrap();
        let mut len = 0;
        let mut buf = buf;
        for frag in data.frags.into_iter() {
            let mut frag = frag.freeze();
            let sz = frag.remaining().min(buf.len());
            frag.copy_to_slice(&mut buf[..sz]);
            buf = &mut buf[sz..];
            len += sz;
            if buf.is_empty() {
                break;
            }
        }
        Ok((len, addr))
    }

    /// Sends data on the socket to the given address. On success, returns the
    /// number of bytes written.
    #[allow(unsafe_code)]
    pub async fn send_to<A: ToSocketAddrs>(&self, buf: &[u8], _addr: A) -> Result<usize> {
        let len = buf.len();
        let l2_sz = mem::size_of::<rte_ether_hdr>();
        let l3_sz = mem::size_of::<rte_ipv4_hdr>();
        let l4_sz = mem::size_of::<rte_udp_hdr>();

        let mut hdr = BytesMut::with_capacity(l2_sz + l3_sz + l4_sz);
        let mut pkt = GenericPacket::new(L2Protocol::Ether, L3Protocol::Ipv4, L4Protocol::UDP);

        // fill l2 header
        let ether_hdr = unsafe { &mut *(hdr.chunk_mut()[..].as_mut_ptr() as *mut rte_ether_hdr) };
        // TODO
        ether_hdr.ether_type = (RTE_ETHER_TYPE_IPV4 as u16).to_be();
        unsafe {
            hdr.advance_mut(l2_sz);
        }

        // fill l3 header
        let _ip_hdr = unsafe { &mut *(hdr.chunk_mut()[..].as_mut_ptr() as *mut rte_ipv4_hdr) };
        // ip_hdr.dst_addr = addr.to_socket_addrs()[0].into();
        unsafe {
            hdr.advance_mut(l3_sz);
        }

        let _udp_hdr = unsafe { &mut *(hdr.chunk_mut()[..].as_mut_ptr() as *mut rte_udp_hdr) };

        pkt.append(hdr);
        pkt.append(BytesMut::from(buf));
        self.tx.send(pkt).await?;
        Ok(len)
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        socket::dealloc_mailbox(self.sockfd);
        socket::free_fd(self.sockfd).unwrap();
    }
}
