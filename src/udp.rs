//! UDP implementation

use crate::{sock, Error, Result};
use std::net::{SocketAddr, ToSocketAddrs};

/// A UDP socket.
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct UdpSocket {
    sockfd: i32,
}

impl UdpSocket {
    /// Creates a UDP socket from the given address.
    pub fn bind<A: ToSocketAddrs>(addr: A) -> Result<Self> {
        while let Some(addr) = addr.to_socket_addrs().unwrap().next() {
            if let Ok(sockfd) = sock::bind_fd(addr) {
                return Ok(UdpSocket { sockfd });
            }
        }
        Err(Error::NoBuf)
    }

    /// Receives a single datagram message on the socket. On success, returns
    /// the number of bytes read and the origin.
    pub fn recv_from(&self, _buf: &mut [u8]) -> Result<(usize, SocketAddr)> {
        todo!()
    }

    /// Sends data on the socket to the given address. On success, returns the
    /// number of bytes written.
    pub fn send_to<A: ToSocketAddrs>(&self, _buf: &[u8], _addr: A) -> Result<usize> {
        todo!()
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        sock::free_fd(self.sockfd).unwrap();
    }
}
