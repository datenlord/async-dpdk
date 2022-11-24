//! Socket implementation

use crate::{packet::Packet, Error, Result};
use lazy_static::lazy_static;
use log::{error, trace};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::{atomic::AtomicU16, Arc, Mutex},
};
use tokio::sync::oneshot;

lazy_static! {
    static ref SOCK_TABLE: SockTable = SockTable::default();
    static ref PORT_TABLE: PortTable = PortTable::default();
    static ref MAILBOX_TABLE: MailboxTable = MailboxTable::default();
    pub(crate) static ref IPID: AtomicU16 = AtomicU16::new(1);
}

/// The max number of sockets a program can open.
const MAX_SOCK_NUM: i32 = 8192;

/// Socket state.
#[derive(Debug, Default, Copy, Clone)]
#[allow(clippy::missing_docs_in_private_items)]
pub(crate) enum SockState {
    #[default]
    Unused,
    InUse {
        port: u16,
    },
}

#[derive(Debug)]
#[allow(clippy::missing_docs_in_private_items)]
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
#[allow(clippy::missing_docs_in_private_items)]
struct SockTableInner {
    /// fd -> SockState
    open: [SockState; MAX_SOCK_NUM as usize],
    /// unused fds
    free_fd: VecDeque<i32>,
}

impl Default for SockTableInner {
    fn default() -> Self {
        Self {
            open: [SockState::default(); MAX_SOCK_NUM as usize],

            free_fd: (0..MAX_SOCK_NUM).into_iter().collect(),
        }
    }
}

#[derive(Debug, Default)]
#[allow(clippy::missing_docs_in_private_items)]
struct PortTable {
    inner: Mutex<PortTableInner>,
}

#[derive(Debug)]
#[allow(clippy::missing_docs_in_private_items)]
struct PortInfo {
    fd: i32,
    ip: IpAddr,
}

#[derive(Debug, Default)]
#[allow(clippy::missing_docs_in_private_items)]
struct PortTableInner {
    /// port -> port info
    info: HashMap<u16, PortInfo>,
    /// the next port available
    next_port: u16,
}

/// Mailboxes for all bound sockets.
#[derive(Debug)]
struct MailboxTable {
    /// fd -> mailbox
    inner: Mutex<BTreeMap<i32, Arc<Mutex<Mailbox>>>>,
}

impl Default for MailboxTable {
    fn default() -> Self {
        Self {
            inner: Mutex::new(BTreeMap::new()),
        }
    }
}

/// The result for trying to receive a packet.
pub(crate) type RecvResult = Result<(SocketAddr, Packet)>;

/// Mailbox is used for packet passing by agents and sockets.
#[derive(Debug, Default)]
pub(crate) struct Mailbox {
    /// Received packets.
    received: VecDeque<RecvResult>,
    /// Registered by sockets.
    watcher: Option<oneshot::Sender<RecvResult>>,
}

impl Mailbox {
    /// Extract a packet from mailbox.
    pub(crate) fn recv(&mut self) -> oneshot::Receiver<RecvResult> {
        let (tx, rx) = oneshot::channel();
        if let Some(res) = self.received.pop_front() {
            trace!("Got a packet from recv buffer");
            #[allow(clippy::unwrap_used)] // rx is impossible to be dropped.
            tx.send(res).unwrap();
        } else {
            trace!("Registered a channel");
            self.watcher = Some(tx);
        }
        rx
    }

    /// Put a packet into mailbox.
    pub(crate) fn put(&mut self, res: RecvResult) {
        trace!("{:?} received a packet", self);
        if let Some(tx) = self.watcher.take() {
            #[allow(clippy::unwrap_used)]
            tx.send(res).unwrap();
        } else {
            self.received.push_back(res);
        }
    }
}

/// Bind sockfd to a (ip, port) pair.
pub(crate) fn bind_fd(addr: SocketAddr) -> Result<(i32, u16)> {
    #[allow(clippy::unwrap_used)]
    let mut inner = SOCK_TABLE.inner.lock().unwrap();
    let fd = inner.free_fd.pop_front().ok_or(Error::NoBuf)?;
    let port = bind_port(addr.port(), addr.ip(), fd)?;
    #[allow(clippy::cast_sign_loss)] // according to libc
    *inner.open.get_mut(fd as usize).ok_or(Error::OutOfRange)? = SockState::InUse { port };
    Ok((fd, port))
}

/// Free the sockfd.
pub(crate) fn free_fd(fd: i32) -> Result<()> {
    if fd < 0 {
        return Err(Error::BadFd);
    }
    #[allow(clippy::unwrap_used)]
    let mut inner = SOCK_TABLE.inner.lock().unwrap();
    #[allow(clippy::cast_sign_loss)] // according to libc
    let state = *inner.open.get(fd as usize).ok_or(Error::OutOfRange)?;
    let port = match state {
        SockState::InUse { port, .. } => port,
        SockState::Unused => 0,
    };
    #[allow(clippy::cast_sign_loss)] // according to libc
    *inner.open.get_mut(fd as usize).ok_or(Error::OutOfRange)? = SockState::Unused;
    inner.free_fd.push_front(fd);
    free_port(port);
    Ok(())
}

/// Bind sockfd to a port, and return the port number.
fn bind_port(port: u16, addr: IpAddr, fd: i32) -> Result<u16> {
    #[allow(clippy::unwrap_used)]
    let mut inner = PORT_TABLE.inner.lock().unwrap();
    #[allow(clippy::integer_arithmetic)] // impossible to underflow
    if inner.info.len() == u16::MAX as usize - 1 {
        error!("Socket number exceeds");
        return Err(Error::NoBuf);
    }
    let port = if port == 0 {
        let mut next_port = inner.next_port.wrapping_add(1);
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
            error!("Port {port} already bound");
            return Err(Error::InvalidArg);
        }
        port
    };
    let info = PortInfo { fd, ip: addr };
    let _prev = inner.info.insert(port, info);
    Ok(port)
}

/// Find a free port.
fn free_port(port: u16) {
    #[allow(clippy::unwrap_used)]
    let _prev = PORT_TABLE.inner.lock().unwrap().info.remove(&port);
}

/// Called by agent thread, find sockfd by (ip, port).
pub(crate) fn addr_2_sockfd(dst_port: u16, dst_ip: IpAddr) -> Option<i32> {
    #[allow(clippy::unwrap_used)]
    let inner = PORT_TABLE.inner.lock().unwrap();
    inner
        .info
        .get(&dst_port)
        .and_then(|&PortInfo { ip, fd }| (ip.is_unspecified() || ip == dst_ip).then(|| fd))
}
/// Called by socket, create mailbox on creation.
pub(crate) fn alloc_mailbox(sockfd: i32) -> Arc<Mutex<Mailbox>> {
    let mailbox = Arc::new(Mutex::new(Mailbox::default()));
    #[allow(clippy::unwrap_used)]
    let _prev = MAILBOX_TABLE
        .inner
        .lock()
        .unwrap()
        .insert(sockfd, Arc::clone(&mailbox));
    mailbox
}

/// Called by socket, destroy mailbox on deletion.
pub(crate) fn dealloc_mailbox(sockfd: i32) {
    #[allow(clippy::unwrap_used)]
    let _prev = MAILBOX_TABLE.inner.lock().unwrap().remove(&sockfd);
}

/// Called by the agent thread, put arrived packets into mailbox.
pub(crate) fn put_mailbox(sockfd: i32, res: RecvResult) {
    #[allow(clippy::unwrap_used)]
    if let Some(mailbox) = MAILBOX_TABLE.inner.lock().unwrap().get(&sockfd) {
        #[allow(clippy::unwrap_used)]
        mailbox.lock().unwrap().put(res);
    }
}
