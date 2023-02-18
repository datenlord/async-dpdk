//! Socket implementation

use crate::{packet::Packet, Result};
use std::net::SocketAddr;

/// The result for trying to receive a packet.
#[allow(dead_code)] // to be fixed in the next PR
pub(crate) type RecvResult = Result<(SocketAddr, Packet)>;

/// Called by the agent thread, put arrived packets into mailbox.
#[allow(dead_code, clippy::needless_pass_by_value, clippy::unnecessary_wraps)] // fix in next PR
pub(crate) fn put_mailbox(_sockfd: i32, _res: RecvResult) -> Result<()> {
    Ok(())
}
