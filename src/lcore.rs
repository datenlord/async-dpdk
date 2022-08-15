//! An module handling lcore.

use crate::*;

/// Get current lcore_id.
#[inline]
pub fn lcore_id() -> u32 {
    #[allow(unsafe_code)]
    unsafe {
        rte_lcore_id()
    }
}
