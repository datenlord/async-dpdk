//! The term `lcore` (i.e. logical core) refers to a DPDK thread. Typically, it is pinned to
//! a physical core to avoid task switching.
//!
//! This module provides some helper functions to check lcore informations such as lcore id,
//! socket id, lcore role, etc.

#![allow(unsafe_code)]
use dpdk_sys::{
    rte_eal_lcore_role, rte_lcore_count, rte_lcore_id_stub, rte_lcore_role_t_ROLE_NON_EAL,
    rte_lcore_role_t_ROLE_OFF, rte_lcore_role_t_ROLE_RTE, rte_lcore_role_t_ROLE_SERVICE,
    rte_socket_count, rte_socket_id,
};

/// Lcore role.
#[derive(Copy, Clone, Debug)]
#[allow(clippy::exhaustive_enums)] // DPDK defined
pub enum Role {
    /// An eal-created thread.
    Eal,
    /// A user-created thread.
    User,
    /// A service lcore.
    Service,
    /// Off.
    Off,
}

/// Get the current `lcore_id`.
#[inline]
#[must_use]
pub fn id() -> u32 {
    // SAFETY: ffi
    unsafe { rte_lcore_id_stub() }
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
pub fn socket_id() -> i32 {
    // SAFETY: ffi
    #[allow(clippy::cast_possible_wrap)] // legal socket_id should be less than i32::MAX
    unsafe {
        rte_socket_id() as i32
    }
}

/// Get socket count.
#[inline]
#[must_use]
pub fn socket_count() -> u32 {
    // SAFETY: ffi
    unsafe { rte_socket_count() }
}
