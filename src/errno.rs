//! DPDK defined error numbers.
use crate::*;

#[inline]
#[allow(unsafe_code)]
pub(crate) fn parse_err(errno: c_int) {
    if errno < 0 {
        let msg = unsafe { rte_strerror(errno) };
        unsafe {
            rte_exit(errno, msg);
        }
    }
}
