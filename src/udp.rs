//! UDP implementation

use crate::{
    eth_dev::TxSender,
    mbuf::Mbuf,
    net_dev,
    packet::Packet,
    proto::{L3Protocol, L4Protocol, Protocol, ETHER_HDR_LEN, IP_NEXT_PROTO_UDP},
    socket::{self, addr_2_sockfd, Mailbox, RecvResult, IPID},
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
#[allow(missing_copy_implementations, clippy::module_name_repetitions)]
pub struct UdpSocket {
    /// Socket fd.
    sockfd: i32,
    /// The IP address that this socket is bound to.
    ip: u32,
    /// The port that this socket is bound to.
    port: u16,
    /// A channel to `TxAgent`.
    tx: TxSender,
    /// A pointer to its mailbox.
    mailbox: Arc<Mutex<Mailbox>>,
    /// ether_addr for the device. TODO remove it
    eth_addr: rte_ether_addr,
}

#[allow(unsafe_code)]
unsafe impl Send for UdpSocket {}

#[allow(unsafe_code)]
unsafe impl Sync for UdpSocket {}

impl UdpSocket {
    /// Creates a UDP socket from the given address.
    ///
    /// # Errors
    ///
    /// Possible reasons:
    ///
    /// - Invalid socket address.
    /// - Too much bound sockets.
    #[inline]
    pub fn bind<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        #[allow(clippy::map_err_ignore)]
        while let Some(addr) = addr
            .to_socket_addrs()
            .map_err(|_| Error::InvalidArg)?
            .next()
        {
            if let Ok((sockfd, port)) = socket::bind_fd(addr) {
                if let Ok((tx, eth_addr)) = net_dev::find_dev_by_ip(addr.ip()) {
                    let mailbox = socket::alloc_mailbox(sockfd)?;
                    let ip = match addr.ip() {
                        IpAddr::V4(addr) => Ok(u32::from_ne_bytes(addr.octets())),
                        // TODO: support ipv6
                        IpAddr::V6(_) => Err(Error::InvalidArg),
                    }?;
                    return Ok(UdpSocket {
                        sockfd,
                        ip,
                        port,
                        tx,
                        mailbox,
                        eth_addr,
                    });
                }
                socket::free_fd(sockfd)?;
                return Err(Error::InvalidArg);
            }
        }
        Err(Error::NoBuf)
    }

    /// Receives a single datagram message on the socket. On success, returns
    /// the number of bytes read and the origin.
    ///
    /// # Errors
    ///
    /// Possible reasons:
    ///
    /// - Recv agent not started.
    #[inline]
    #[allow(clippy::indexing_slicing)]
    pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        let rx = self.mailbox.lock().map_err(Error::from)?.recv()?;
        let (addr, data) = rx.await.map_err(Error::from)??;
        let mut len: usize = 0;
        let mut buf = buf;
        for frag in data.frags {
            let mut frag = frag.freeze();
            let sz = frag.remaining().min(buf.len());
            frag.copy_to_slice(&mut buf[..sz]); // TODO zero-copy
            buf = &mut buf[sz..];
            len = len.wrapping_add(sz);
            if buf.is_empty() {
                break;
            }
        }
        Ok((len, addr))
    }

    /// Sends data on the socket to the given address. On success, returns the
    /// number of bytes written.
    ///
    /// # Errors
    ///
    /// Possible reasons:
    ///
    /// - Invalid socket address.
    /// - Data to long.
    /// - Send agent not started.
    #[inline]
    #[allow(unsafe_code, clippy::cast_possible_truncation)]
    pub async fn send_to<A: ToSocketAddrs>(&self, buf: &[u8], addr: A) -> Result<usize> {
        #[allow(clippy::map_err_ignore)]
        let addr = addr
            .to_socket_addrs()
            .map_err(|_| Error::InvalidArg)?
            .next()
            .ok_or(Error::InvalidArg)?;

        let buf_len = buf.len();
        let l2_sz = ETHER_HDR_LEN;
        let l3_sz = L3Protocol::Ipv4.length();
        let l4_sz = L4Protocol::Udp.length();
        let payload_len: u16 = buf.len().try_into().map_err(Error::from)?;
        let total_len = payload_len
            .checked_add(l3_sz)
            .ok_or(Error::InvalidArg)?
            .checked_add(l4_sz)
            .ok_or(Error::InvalidArg)?;

        let mut hdr = BytesMut::with_capacity(l2_sz.wrapping_add(l3_sz).wrapping_add(l4_sz) as _);
        let mut pkt = Packet::new(L3Protocol::Ipv4, L4Protocol::Udp);

        // make this function `Send`.
        {
            // fill l2 header
            // SAFETY: hdr size = l2_sz + l3_sz + l4_sz
            #[allow(clippy::cast_ptr_alignment)]
            let ether_hdr =
                unsafe { &mut *(hdr.chunk_mut()[..].as_mut_ptr().cast::<rte_ether_hdr>()) };
            ether_hdr.src_addr = self.eth_addr;
            // TODO send to real mac addr. implement ARP in the future!
            ether_hdr.dst_addr.addr_bytes.copy_from_slice(&[0xff; 6]);
            ether_hdr.ether_type = (RTE_ETHER_TYPE_IPV4 as u16).to_be();

            // SAFETY: hdr size = l2_sz + l3_sz + l4_sz
            unsafe {
                hdr.advance_mut(l2_sz as _);
            }

            // fill l3 header
            // SAFETY: hdr size = l2_sz + l3_sz + l4_sz
            let ip_hdr = unsafe { &mut *(hdr.chunk_mut()[..].as_mut_ptr().cast::<rte_ipv4_hdr>()) };
            ip_hdr.version_ihl_union.version_ihl = 0x45; // version = 4, ihl = 5
            ip_hdr.type_of_service = 0;
            ip_hdr.total_length = total_len.to_be();

            ip_hdr.packet_id = IPID.fetch_add(1, Ordering::AcqRel).to_be();
            ip_hdr.fragment_offset = 0u16;
            ip_hdr.time_to_live = 64;
            ip_hdr.next_proto_id = IP_NEXT_PROTO_UDP;
            ip_hdr.dst_addr = match addr.ip() {
                IpAddr::V4(addr) => u32::from_ne_bytes(addr.octets()),
                #[allow(clippy::unimplemented)]
                IpAddr::V6(_) => unimplemented!(),
            };
            ip_hdr.src_addr = self.ip;
            // SAFETY: ffi
            ip_hdr.hdr_checksum = unsafe { rte_ipv4_cksum(ip_hdr).to_be() };

            // SAFETY: hdr size = l2_sz + l3_sz + l4_sz
            unsafe {
                hdr.advance_mut(l3_sz as _);
            }

            // SAFETY: hdr size = l2_sz + l3_sz + l4_sz
            let udp_hdr = unsafe { &mut *(hdr.chunk_mut()[..].as_mut_ptr().cast::<rte_udp_hdr>()) };
            udp_hdr.src_port = self.port;
            udp_hdr.dst_port = addr.port();
            udp_hdr.dgram_len = payload_len.wrapping_add(l4_sz).to_be();
            udp_hdr.dgram_cksum = 0;

            // SAFETY: hdr size = l2_sz + l3_sz + l4_sz
            unsafe {
                hdr.advance_mut(l4_sz as _);
            }
            pkt.append(hdr);
            pkt.append(BytesMut::from(buf));
        }
        self.tx.send(pkt).await?;
        Ok(buf_len)
    }
}

impl Debug for UdpSocket {
    #[inline]
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
    #[inline]
    fn drop(&mut self) {
        #[allow(clippy::unwrap_used)] // used in drop
        socket::dealloc_mailbox(self.sockfd).unwrap();
        #[allow(clippy::unwrap_used)] // used in drop
        socket::free_fd(self.sockfd).unwrap();
    }
}

/// Handle IPv4 & UDP packet.
///
/// Information such as IP + port of source and destination will be parsed,
/// and the packet will be put into the corresponding `Mailbox`.
pub(crate) fn handle_ipv4_udp(mut m: Mbuf) -> Option<(i32, RecvResult)> {
    // Parse IPv4 and UDP header.
    let data = m.data_slice();

    // SAFETY: remain size larger than `rte_ipv4_hdr`, which is checked in `handle_ether`
    #[allow(unsafe_code)]
    let ip_hdr = unsafe { &*(data.as_ptr().cast::<rte_ipv4_hdr>()) };
    let dst_ip_bytes: [u8; 4] = ip_hdr.dst_addr.to_ne_bytes();
    let dst_ip = IpAddr::from(dst_ip_bytes);
    let src_ip_bytes: [u8; 4] = ip_hdr.src_addr.to_ne_bytes();
    let src_ip = IpAddr::from(src_ip_bytes);
    log::debug!("from {src_ip:?} to {dst_ip:?}");

    if data.len()
        < L3Protocol::Ipv4
            .length()
            .saturating_add(L4Protocol::Udp.length()) as usize
    {
        return None;
    }

    // SAFETY: remain size larger than `rte_udp_hdr` size
    #[allow(unsafe_code, trivial_casts)]
    let udp_hdr = unsafe { &*((ip_hdr as *const rte_ipv4_hdr).add(1).cast::<rte_udp_hdr>()) };
    let dst_port = udp_hdr.dst_port;
    let src_port = udp_hdr.src_port;
    let _len = udp_hdr.dgram_len.to_be();
    let src_addr = SocketAddr::new(src_ip, src_port);

    let hdr_len = L3Protocol::Ipv4
        .length()
        .saturating_add(L4Protocol::Udp.length());
    m.adj(hdr_len as _).ok()?;
    let packet = Packet::from_mbuf(m);

    if let Some(sockfd) = addr_2_sockfd(dst_port, dst_ip) {
        return Some((sockfd, Ok((src_addr, packet))));
    }
    log::warn!("sockfd not found: {dst_ip:?}:{dst_port}");
    None
}
