//! Socket implementation

use crate::{Error, Result};
use lazy_static::lazy_static;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Mutex,
};

lazy_static! {
    static ref SOCK_TABLE: SockTable = SockTable::default();
    static ref PORT_TABLE: PortTable = PortTable::default();
}

const MAX_SOCK_NUM: usize = 1024;

#[derive(Debug, Default, Copy, Clone)]
pub(crate) enum SockState {
    #[default]
    Unused,
    InUse {
        ip: IpAddr,
        port: u16,
        #[allow(dead_code)]
        op: i32,
    },
}

#[derive(Debug)]
struct SockTable {
    inner: Mutex<SockTableInner>,
}

impl Default for SockTable {
    fn default() -> Self {
        Self {
            inner: Mutex::new(SockTableInner::default()),
        }
    }
}

#[derive(Debug)]
struct SockTableInner {
    open: [SockState; MAX_SOCK_NUM],
    free_fd: Vec<i32>,
}

impl Default for SockTableInner {
    fn default() -> Self {
        Self {
            open: [SockState::default(); MAX_SOCK_NUM],
            free_fd: (0..MAX_SOCK_NUM).into_iter().map(|i| i as i32).collect(),
        }
    }
}

#[derive(Debug, Default)]
struct PortTable {
    inner: Mutex<PortTableInner>,
}

#[derive(Debug, Default)]
struct PortTableInner {
    fd: HashMap<u16, i32>, // port -> fd map
    next_port: u16,
}

// Bind sockfd to a (ip, port) pair.
pub(crate) fn bind_fd(addr: SocketAddr) -> Result<i32> {
    let mut inner = SOCK_TABLE.inner.lock().unwrap();
    let fd = inner.free_fd.pop().ok_or(Error::NoBuf)?;
    let port = bind_port(addr.port(), fd)?;
    inner.open[fd as usize] = SockState::InUse {
        ip: addr.ip(),
        port: port,
        op: 0,
    };
    Ok(fd)
}

// Free the sockfd.
pub(crate) fn free_fd(fd: i32) -> Result<()> {
    if fd < 0 {
        return Err(Error::BadFd);
    }
    let mut inner = SOCK_TABLE.inner.lock().unwrap();
    let port = match inner.open[fd as usize] {
        SockState::InUse { port, .. } => port,
        _ => 0,
    };
    inner.open[fd as usize] = SockState::Unused;
    inner.free_fd.push(fd);
    free_port(port);
    Ok(())
}

// Bind sockfd to a port, and return the port number.
fn bind_port(port: u16, fd: i32) -> Result<u16> {
    let mut inner = PORT_TABLE.inner.lock().unwrap();
    if inner.fd.len() == u16::MAX as usize - 1 {
        return Err(Error::NoBuf);
    }
    let port = if port == 0 {
        let mut next_port = inner.next_port + 1;
        while inner.fd.get(&next_port).is_some() {
            next_port = next_port.wrapping_add(1);
            if next_port == 0 {
                next_port = 1;
            }
        }
        inner.next_port = next_port;
        next_port
    } else {
        // check if this port is already bound
        if inner.fd.get(&port).is_some() {
            return Err(Error::InvalidArg);
        }
        port
    };
    let _ = inner.fd.insert(port, fd);
    Ok(port)
}

fn free_port(port: u16) {
    let mut inner = PORT_TABLE.inner.lock().unwrap();
    let _ = inner.fd.remove(&port);
}
