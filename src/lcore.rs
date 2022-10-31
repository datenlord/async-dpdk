//! An module handling lcore.

#![allow(unsafe_code)]
use dpdk_sys::{
    rte_eal_lcore_role, rte_lcore_count, rte_lcore_id, rte_lcore_role_t_ROLE_NON_EAL,
    rte_lcore_role_t_ROLE_OFF, rte_lcore_role_t_ROLE_RTE, rte_lcore_role_t_ROLE_SERVICE,
    rte_socket_count, rte_socket_id,
};

/// Lcore role.
#[repr(u32)]
#[non_exhaustive]
#[derive(Copy, Clone, Debug)]
pub enum Role {
    /// An eal-created thread.
    Eal = rte_lcore_role_t_ROLE_RTE,
    /// A user-created thread.
    User = rte_lcore_role_t_ROLE_NON_EAL,
    /// A service lcore.
    Service = rte_lcore_role_t_ROLE_SERVICE,
    /// Off.
    Off = rte_lcore_role_t_ROLE_OFF,
}

/// Get current `lcore_id`.
#[inline]
#[must_use]
pub fn id() -> u32 {
    // SAFETY: ffi
    unsafe { rte_lcore_id() }
}

/// Get lcore count.
#[inline]
#[must_use]
pub fn count() -> u32 {
    // SAFETY: ffi
    unsafe { rte_lcore_count() }
}

/// Get lcore role.
#[inline]
#[allow(non_upper_case_globals)]
#[must_use]
pub fn role(lcore_id: u32) -> Role {
    // SAFETY: ffi
    let role = unsafe { rte_eal_lcore_role(lcore_id) };
    match role {
        rte_lcore_role_t_ROLE_RTE => Role::Eal,
        rte_lcore_role_t_ROLE_SERVICE => Role::Service,
        rte_lcore_role_t_ROLE_NON_EAL => Role::User,
        rte_lcore_role_t_ROLE_OFF => Role::Off,
        _ => unreachable!(),
    }
}

/// Get current socket id.
#[inline]
#[must_use]
pub fn socket_id() -> u32 {
    // SAFETY: ffi
    unsafe { rte_socket_id() }
}

/// Get socket count.
#[inline]
#[must_use]
pub fn socket_count() -> u32 {
    // SAFETY: ffi
    unsafe { rte_socket_count() }
}
