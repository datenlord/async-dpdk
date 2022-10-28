//! UDP implementation

use crate::{
    eth_dev::TxSender,
    mbuf::Mbuf,
    net_dev,
    packet::Packet,
    protocol::{L3Protocol, L4Protocol, Protocol, ETHER_HDR_LEN, IP_NEXT_PROTO_UDP},
    socket::{self, addr_2_sockfd, Mailbox, IPID},
    Error, Result,
};
use bytes::{Buf, BufMut, BytesMut};
use dpdk_sys::{
    rte_ether_addr, rte_ether_hdr, rte_ipv4_cksum, rte_ipv4_hdr, rte_udp_hdr, RTE_ETHER_TYPE_IPV4,
};
use std::{
    fmt::Debug,
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::{atomic::Ordering, Arc, Mutex},
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

#[allow(unsafe_code)]
unsafe impl Send for UdpSocket {}

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
                return Err(Error::InvalidArg);
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

        let mut hdr = BytesMut::with_capacity(l2_sz + l3_sz + l4_sz);
        let mut pkt = Packet::new(L3Protocol::Ipv4, L4Protocol::UDP);

        // fill header
        {
            // fill l2 header
            let ether_hdr =
                unsafe { &mut *(hdr.chunk_mut()[..].as_mut_ptr() as *mut rte_ether_hdr) };
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
            ip_hdr.packet_id = IPID.fetch_add(1, Ordering::AcqRel).to_be();
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
        }
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

    let ip_hdr = unsafe { &*(data.as_ptr() as *const rte_ipv4_hdr) };
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

    let hdr_len = L3Protocol::Ipv4.length() + L4Protocol::UDP.length();
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

#[cfg(test)]
mod test_server_client {
    use super::UdpSocket;
    use crate::{eal, net_dev};
    use std::net::{IpAddr, SocketAddr};
    use tokio::task;

    const MSG: &'static str = "this is client message";
    const ACK: &'static str = "this is ack message";

    async fn server() {
        let socket = UdpSocket::bind("10.2.3.0:1234").unwrap();
        let mut buffer = [0u8; 30];
        let (sz, client_addr) = socket.recv_from(&mut buffer).await.unwrap();
        assert_eq!(sz, MSG.len());
        assert_eq!(client_addr.ip(), IpAddr::from([10, 2, 3, 1]));
        assert_eq!(&buffer[..sz], MSG.as_bytes());
        let sz = socket.send_to(ACK.as_bytes(), client_addr).await.unwrap();
        assert_eq!(sz, ACK.len());
    }

    async fn client() {
        let socket = UdpSocket::bind("10.2.3.1:0").unwrap();
        let mut buffer = [0u8; 30];
        let sz = socket
            .send_to(MSG.as_bytes(), "10.2.3.0:1234")
            .await
            .unwrap();
        assert_eq!(sz, MSG.len());
        let (sz, server_addr) = socket.recv_from(&mut buffer).await.unwrap();
        assert_eq!(sz, ACK.len());
        assert_eq!(server_addr, SocketAddr::from(([10, 2, 3, 0], 1234)));
        assert_eq!(&buffer[..sz], ACK.as_bytes());
    }

    #[tokio::test]
    async fn test() {
        eal::Builder::new().enter().unwrap();
        net_dev::device_probe(&["10.2.3.0", "10.2.3.1"]).unwrap();
        let server = task::spawn(server());
        client().await;
        server.await.unwrap();
        net_dev::device_close().unwrap();
    }
}

#[cfg(test)]
mod test_multi_clients {
    use super::UdpSocket;
    use crate::{eal, net_dev};
    use tokio::task;

    async fn server() {
        let socket = UdpSocket::bind("10.2.3.0:1234").unwrap();
        let mut buffer = [0u8; 30];
        for _ in 0..2 {
            let (sz, addr) = socket.recv_from(&mut buffer).await.unwrap();
            let _sz = socket.send_to(&buffer[..sz], addr).await.unwrap();
        }
    }

    async fn client(number: i32) {
        let msg = format!("my client number is {}", number);
        let socket = UdpSocket::bind("10.2.3.1:0").unwrap();
        let mut buffer = [0u8; 30];
        let _sz = socket
            .send_to(msg.as_bytes(), "10.2.3.0:1234")
            .await
            .unwrap();
        let _ = socket.recv_from(&mut buffer).await.unwrap();
    }

    #[tokio::test]
    async fn test() {
        eal::Builder::new().enter().unwrap();
        net_dev::device_probe(&["10.2.3.0", "10.2.3.1"]).unwrap();
        let server = task::spawn(server());
        client(0).await;
        client(1).await;
        server.await.unwrap();
        net_dev::device_close().unwrap();
    }
}

#[cfg(test)]
mod test_frag {
    use super::UdpSocket;
    use crate::{
        eal::{self, LogLevel},
        net_dev,
    };
    use tokio::task;

    const LEN: usize = 2000; // > Ethernet MTU

    async fn server() {
        let socket = UdpSocket::bind("10.2.3.0:1234").unwrap();
        let mut buffer = [0u8; LEN];
        let (sz, _addr) = socket.recv_from(&mut buffer).await.unwrap();
        assert_eq!(sz, LEN);
        assert_eq!(buffer[1], 1);
    }

    async fn client() {
        let socket = UdpSocket::bind("10.2.3.1:0").unwrap();
        let buffer = [1u8; LEN];
        let sz = socket.send_to(&buffer[..], "10.2.3.0:1234").await.unwrap();
        assert_eq!(sz, LEN);
    }
    #[tokio::test]
    async fn test() {
        eal::Builder::new()
            .log_level(LogLevel::Debug)
            .enter()
            .unwrap();
        net_dev::device_probe(&["10.2.3.0", "10.2.3.1"]).unwrap();
        let server = task::spawn(server());
        client().await;
        server.await.unwrap();
        net_dev::device_close().unwrap();
    }
}
