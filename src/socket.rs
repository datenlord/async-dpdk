//! Socket implementation

use crate::{packet::Packet, Error, Result};
use lazy_static::lazy_static;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;

lazy_static! {
    static ref SOCK_TABLE: SockTable = SockTable::default();
    static ref PORT_TABLE: PortTable = PortTable::default();
    static ref MAILBOX_TABLE: MailboxTable = MailboxTable::default();
}

const MAX_SOCK_NUM: usize = 1024;

#[derive(Debug, Default, Copy, Clone)]
pub(crate) enum SockState {
    #[default]
    Unused,
    InUse {
        port: u16,
        #[allow(dead_code)]
        op: i32, // TODO socket op
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
    free_fd: VecDeque<i32>,
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

#[derive(Debug)]
#[allow(dead_code)]
struct PortInfo {
    fd: i32, // sockfd
    ip: IpAddr,
}

#[derive(Debug, Default)]
struct PortTableInner {
    info: HashMap<u16, PortInfo>,
    next_port: u16,
}

#[derive(Debug)]
struct MailboxTable {
    inner: Mutex<BTreeMap<i32, Arc<Mutex<Mailbox>>>>,
}

impl Default for MailboxTable {
    fn default() -> Self {
        Self {
            inner: Mutex::new(BTreeMap::new()),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Mailbox {
    received: VecDeque<(SocketAddr, Packet)>,
    watcher: Option<oneshot::Sender<(SocketAddr, Packet)>>,
}

impl Mailbox {
    pub(crate) fn recv(&mut self) -> oneshot::Receiver<(SocketAddr, Packet)> {
        let (tx, rx) = oneshot::channel();
        if let Some((addr, data)) = self.received.pop_front() {
            tx.send((addr, data)).unwrap();
        } else {
            self.watcher = Some(tx);
        }
        rx
    }

    pub(crate) fn put(&mut self, addr: SocketAddr, data: Packet) {
        if let Some(tx) = self.watcher.take() {
            tx.send((addr, data)).unwrap();
        } else {
            self.received.push_back((addr, data));
        }
    }
}

// Bind sockfd to a (ip, port) pair.
pub(crate) fn bind_fd(addr: SocketAddr) -> Result<(i32, u16)> {
    let mut inner = SOCK_TABLE.inner.lock().unwrap();
    let fd = inner.free_fd.pop_front().ok_or(Error::NoBuf)?;
    let port = bind_port(addr.port(), addr.ip(), fd)?;
    inner.open[fd as usize] = SockState::InUse { port, op: 0 };
    Ok((fd, port))
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
    inner.free_fd.push_front(fd);
    free_port(port);
    Ok(())
}

// Bind sockfd to a port, and return the port number.
fn bind_port(port: u16, addr: IpAddr, fd: i32) -> Result<u16> {
    let mut inner = PORT_TABLE.inner.lock().unwrap();
    if inner.info.len() == u16::MAX as usize - 1 {
        return Err(Error::NoBuf);
    }
    let port = if port == 0 {
        let mut next_port = inner.next_port + 1;
        while inner.info.get(&next_port).is_some() {
            next_port = next_port.wrapping_add(1);
            if next_port == 0 {
                next_port = 1;
            }
        }
        inner.next_port = next_port;
        next_port
    } else {
        // check if this port is already bound
        if inner.info.get(&port).is_some() {
            return Err(Error::InvalidArg);
        }
        port
    };
    let info = PortInfo { fd, ip: addr };
    let _ = inner.info.insert(port, info);
    Ok(port)
}

fn free_port(port: u16) {
    let _ = PORT_TABLE.inner.lock().unwrap().info.remove(&port);
}

pub(crate) fn addr_2_sockfd(dst_port: u16, dst_ip: IpAddr) -> Option<i32> {
    let inner = PORT_TABLE.inner.lock().unwrap();
    inner
        .info
        .get(&dst_port)
        .and_then(|&PortInfo { ip, fd, .. }| {
            if ip.is_unspecified() || ip == dst_ip {
                Some(fd)
            } else {
                None
            }
        })
}

pub(crate) fn alloc_mailbox(sockfd: i32) -> Arc<Mutex<Mailbox>> {
    let mailbox = Arc::new(Mutex::new(Mailbox::default()));
    let _ = MAILBOX_TABLE
        .inner
        .lock()
        .unwrap()
        .insert(sockfd, mailbox.clone());
    mailbox
}

pub(crate) fn dealloc_mailbox(sockfd: i32) {
    let _ = MAILBOX_TABLE.inner.lock().unwrap().remove(&sockfd);
}

pub(crate) fn put_mailbox(sockfd: i32, addr: SocketAddr, data: Packet) {
    if let Some(mailbox) = MAILBOX_TABLE.inner.lock().unwrap().get(&sockfd) {
        mailbox.lock().unwrap().put(addr, data);
    }
}
