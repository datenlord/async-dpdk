//! UDP implementation

use crate::{mbuf::Mbuf, socket::RecvResult};

/// Handle IPv4 & UDP packet.
///
/// Information such as IP + port of source and destination will be parsed,
/// and the packet will be put into the corresponding `Mailbox`.
#[allow(clippy::needless_pass_by_value)] // fix in next PR
pub(crate) fn handle_ipv4_udp(_m: Mbuf) -> Option<(i32, RecvResult)> {
    None
}
