//! An module handling lcore.

#![allow(unsafe_code)]
use crate::*;

/// Lcore role.
#[repr(u32)]
#[derive(Copy, Clone, Debug)]
pub enum LcoreRole {
    /// An eal-created thread.
    Eal = rte_lcore_role_t_ROLE_RTE,
    /// A user-created thread.
    User = rte_lcore_role_t_ROLE_NON_EAL,
    /// A service lcore.
    Service = rte_lcore_role_t_ROLE_SERVICE,
    /// Off.
    Off = rte_lcore_role_t_ROLE_OFF,
}

/// Get current lcore_id.
#[inline]
pub fn lcore_id() -> u32 {
    // SAFETY: ffi
    unsafe { rte_lcore_id() }
}

/// Get lcore count.
#[inline]
pub fn lcore_count() -> u32 {
    // SAFETY: ffi
    unsafe { rte_lcore_count() }
}

/// Get lcore role.
#[inline]
#[allow(non_upper_case_globals)]
pub fn lcore_role(lcore_id: u32) -> LcoreRole {
    // SAFETY: ffi
    let role = unsafe { rte_eal_lcore_role(lcore_id) };
    match role {
        rte_lcore_role_t_ROLE_RTE => LcoreRole::Eal,
        rte_lcore_role_t_ROLE_SERVICE => LcoreRole::Service,
        rte_lcore_role_t_ROLE_NON_EAL => LcoreRole::User,
        rte_lcore_role_t_ROLE_OFF => LcoreRole::Off,
        _ => unreachable!(),
    }
}

/// Get current socket id.
#[inline]
pub fn socket_id() -> u32 {
    // SAFETY: ffi
    unsafe { rte_socket_id() }
}

/// Get socket count.
#[inline]
pub fn socket_count() -> u32 {
    // SAFETY: ffi
    unsafe { rte_socket_count() }
}
