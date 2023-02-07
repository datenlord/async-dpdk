//! UDP implementation

use crate::{mbuf::Mbuf, socket::RecvResult};

/// Handle IPv4 packet.
#[allow(clippy::needless_pass_by_value)] // fix in next PR
pub(crate) fn handle_ipv4_udp(_m: Mbuf) -> Option<(i32, RecvResult)> {
    None
}
