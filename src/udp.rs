//! UDP implementation

use crate::{
    eth_dev::TxSender,
    mbuf::Mbuf,
    net_dev,
    packet::Packet,
    protocol::{L3Protocol, L4Protocol, Protocol, ETHER_HDR_LEN, IP_NEXT_PROTO_UDP},
    socket::{self, addr_2_sockfd, Mailbox},
    Error, Result,
};
use bytes::{Buf, BufMut, BytesMut};
use dpdk_sys::{
    rte_ether_addr, rte_ether_hdr, rte_ipv4_cksum, rte_ipv4_hdr, rte_udp_hdr, RTE_ETHER_TYPE_IPV4,
};
use std::{
    fmt::Debug,
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::{Arc, Mutex},
};

/// A UDP socket.
#[allow(missing_copy_implementations)]
pub struct UdpSocket {
    sockfd: i32,
    ip: u32,
    port: u16,
    tx: TxSender,
    mailbox: Arc<Mutex<Mailbox>>,
    eth_addr: rte_ether_addr,
}

impl UdpSocket {
    /// Creates a UDP socket from the given address.
    pub fn bind<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        while let Some(addr) = addr.to_socket_addrs().unwrap().next() {
            if let Ok((sockfd, port)) = socket::bind_fd(addr) {
                if let Ok((tx, eth_addr)) = net_dev::find_dev_by_ip(addr.ip()) {
                    let mailbox = socket::alloc_mailbox(sockfd);
                    let ip = match addr.ip() {
                        IpAddr::V4(addr) => u32::from_ne_bytes(addr.octets()),
                        IpAddr::V6(_) => todo!(),
                    };
                    return Ok(UdpSocket {
                        sockfd,
                        ip,
                        port,
                        tx,
                        mailbox,
                        eth_addr,
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
    pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
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
    pub async fn send_to<A: ToSocketAddrs>(&self, buf: &[u8], addr: A) -> Result<usize> {
        let addr = addr
            .to_socket_addrs()
            .map_err(|_| Error::InvalidArg)?
            .next()
            .ok_or(Error::InvalidArg)?;
        let len = buf.len();
        let l2_sz = ETHER_HDR_LEN;
        let l3_sz = L3Protocol::Ipv4.length();
        let l4_sz = L4Protocol::UDP.length();

        // TODO send - consider fragmentation

        let mut hdr = BytesMut::with_capacity(l2_sz + l3_sz + l4_sz);
        let mut pkt = Packet::new(L3Protocol::Ipv4, L4Protocol::UDP);

        // fill l2 header
        let ether_hdr = unsafe { &mut *(hdr.chunk_mut()[..].as_mut_ptr() as *mut rte_ether_hdr) };
        ether_hdr.src_addr = self.eth_addr;
        // TODO send to real mac addr. implement ARP in the future!
        ether_hdr.dst_addr.addr_bytes.copy_from_slice(&[0xff; 6]);
        ether_hdr.ether_type = (RTE_ETHER_TYPE_IPV4 as u16).to_be();
        unsafe {
            hdr.advance_mut(l2_sz);
        }

        // fill l3 header
        let ip_hdr = unsafe { &mut *(hdr.chunk_mut()[..].as_mut_ptr() as *mut rte_ipv4_hdr) };
        ip_hdr.version_ihl_union.version_ihl = 0x45; // version = 4, ihl = 5
        ip_hdr.type_of_service = 0;
        ip_hdr.total_length = ((buf.len() + l4_sz + l3_sz) as u16).to_be();
        ip_hdr.packet_id = 0u16.to_be(); // TODO some meaningful data
        ip_hdr.fragment_offset = 0u16.to_be();
        ip_hdr.time_to_live = 64;
        ip_hdr.next_proto_id = IP_NEXT_PROTO_UDP;
        ip_hdr.dst_addr = match addr.ip() {
            IpAddr::V4(addr) => u32::from_ne_bytes(addr.octets()),
            IpAddr::V6(_) => unimplemented!(),
        };
        ip_hdr.src_addr = self.ip;
        // SAFETY: ffi
        ip_hdr.hdr_checksum = unsafe { rte_ipv4_cksum(ip_hdr).to_be() };
        unsafe {
            hdr.advance_mut(l3_sz);
        }

        let udp_hdr = unsafe { &mut *(hdr.chunk_mut()[..].as_mut_ptr() as *mut rte_udp_hdr) };
        udp_hdr.src_port = self.port;
        udp_hdr.dst_port = addr.port();
        udp_hdr.dgram_len = ((buf.len() + l4_sz) as u16).to_be();
        udp_hdr.dgram_cksum = 0;
        unsafe {
            hdr.advance_mut(l4_sz);
        }

        pkt.append(hdr);
        pkt.append(BytesMut::from(buf));
        self.tx.send(pkt).await?;
        Ok(len)
    }
}

impl Debug for UdpSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UdpSocket")
            .field("sockfd", &self.sockfd)
            .field("ip", &self.ip)
            .field("port", &self.port)
            .field("tx", &self.tx)
            .finish()
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        socket::dealloc_mailbox(self.sockfd);
        socket::free_fd(self.sockfd).unwrap();
    }
}

#[allow(unsafe_code)]
pub(crate) fn handle_ipv4_udp(mut m: Mbuf) {
    // Parse IPv4 and UDP header.
    let data = m.data_slice();

    let eth_hdr = unsafe { &*(data.as_ptr() as *const rte_ether_hdr) };

    #[allow(trivial_casts)]
    let ip_hdr = unsafe { &*((eth_hdr as *const rte_ether_hdr).add(1) as *const rte_ipv4_hdr) };
    let dst_ip_bytes: [u8; 4] = ip_hdr.dst_addr.to_ne_bytes();
    let dst_ip = IpAddr::from(dst_ip_bytes);
    let src_ip_bytes: [u8; 4] = ip_hdr.src_addr.to_ne_bytes();
    let src_ip = IpAddr::from(src_ip_bytes);

    #[allow(trivial_casts)]
    let udp_hdr = unsafe { &*((ip_hdr as *const rte_ipv4_hdr).add(1) as *const rte_udp_hdr) };
    let dst_port = udp_hdr.dst_port;
    let src_port = udp_hdr.src_port;
    let _len = udp_hdr.dgram_len.to_be();
    let src_addr = SocketAddr::new(src_ip, src_port);

    let hdr_len = ETHER_HDR_LEN + L3Protocol::Ipv4.length() + L4Protocol::UDP.length();
    m.adj(hdr_len).unwrap();
    let packet = Packet::from_mbuf(m).unwrap();

    if let Some(sockfd) = addr_2_sockfd(dst_port, dst_ip) {
        socket::put_mailbox(sockfd, src_addr, packet);
    } else {
        eprintln!("sockfd not found: {dst_ip:?}:{dst_port}");
    }
}

#[allow(unsafe_code)]
pub(crate) fn handle_ipv6_udp(_m: Mbuf) {
    todo!()
}
