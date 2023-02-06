//! DPDK EAL facilitates the reservation of huge-page memory zones. By default, the EAL reserves
//! hugepages as soon as the app launches, which will be returned to the system when the app ends.
//! This module provides APIs for the allocation and deallocation of DPDK reserved memory. Also,
//! users should be careful to deallocate the applied memory back to DPDK memory subsystem when
//! the usage is over.
//!
//! There are two modes in which DPDK memory subsystem can operate: dynamic mode and legacy mode.
//! For more details, please check
//! #[DPDK document](https://doc.dpdk.org/guides/prog_guide/env_abstraction_layer.html#memory-mapping-discovery-and-memory-reservation)
//! for more details.
//!
//! # Examples
//!
//! ```
//! eal::Config::new().iova_mode(IovaMode::VA).enter().unwrap();
//! let t = alloc::malloc::<Test>();
//! unsafe {
//!     alloc::free(t);
//! }
//! ```

use dpdk_sys::{rte_free, rte_malloc, rte_malloc_socket, rte_zmalloc, rte_zmalloc_socket};
use std::{mem, ptr};

/// This function allocates memory from the huge-page area of memory. The memory is not initialized.
/// In NUMA systems, the memory allocated resides on the same NUMA socket as the core that calls this
/// function.
#[inline]
#[must_use]
pub fn malloc<T: Default>() -> Box<T> {
    // SAFETY: ffi
    #[allow(unsafe_code)]
    unsafe {
        let ptr = rte_malloc(ptr::null(), mem::size_of::<T>(), 0);
        *ptr.cast::<T>() = T::default();
        Box::from_raw(ptr.cast())
    }
}

/// Allocate zeroed memory from the heap. In NUMA systems, the memory allocated resides on the same NUMA socket
/// as the core that calls this function.
#[inline]
#[must_use]
pub fn zmalloc<T>() -> Box<T> {
    // SAFETY: ffi
    #[allow(unsafe_code)]
    unsafe {
        let ptr = rte_zmalloc(ptr::null(), mem::size_of::<T>(), 0);
        Box::from_raw(ptr.cast())
    }
}

/// Allocate memory from hugepages on specific socket.
#[inline]
#[must_use]
pub fn malloc_socket<T: Default>(socket: i32) -> Box<T> {
    // SAFETY: ffi
    #[allow(unsafe_code)]
    unsafe {
        let ptr = rte_malloc_socket(ptr::null(), mem::size_of::<T>(), 0, socket);
        *ptr.cast::<T>() = T::default();
        Box::from_raw(ptr.cast())
    }
}

/// Allocate zeroed memory from hugepages on specific socket.
#[inline]
#[must_use]
pub fn zmalloc_socket<T>(socket: i32) -> Box<T> {
    // SAFETY: ffi
    #[allow(unsafe_code)]
    unsafe {
        let ptr = rte_zmalloc_socket(ptr::null(), mem::size_of::<T>(), 0, socket);
        Box::from_raw(ptr.cast())
    }
}

/// Frees the memory space pointed to by the provided pointer. This pointer must have been returned
/// by a previous call to `malloc()`, `zmalloc()`, `malloc_socket()` or `zmalloc_socket()`.
///
/// If the pointer is NULL, the function does nothing.
///
/// # Safety
///
/// The behaviour of `free()` is undefined if the memory is not allocated by DPDK.
#[inline]
#[allow(unsafe_code)]
pub unsafe fn free<T>(obj: Box<T>) {
    let ptr = Box::into_raw(obj);
    // SAFETY: ffi
    #[allow(unsafe_code)]
    unsafe {
        rte_free(ptr.cast());
    }
}

#[cfg(test)]
mod tests {
    use crate::alloc;
    use crate::eal::{self, IovaMode, LogLevel};

    #[test]
    fn test() {
        #[repr(C)]
        struct Test {
            x: i32,
            y: i64,
        }
        impl Default for Test {
            fn default() -> Self {
                Self { x: 1, y: 2 }
            }
        }

        eal::Config::new()
            .log_level(LogLevel::Debug)
            .iova_mode(IovaMode::VA)
            .enter()
            .unwrap();

        let t1 = alloc::malloc::<Test>();
        assert_eq!(t1.x, 1);
        assert_eq!(t1.y, 2);

        let t2 = alloc::zmalloc::<Test>();
        assert_eq!(t2.x, 0);
        assert_eq!(t2.y, 0);

        let t3 = alloc::malloc_socket::<Test>(0);
        assert_eq!(t3.x, 1);
        assert_eq!(t3.y, 2);

        let t4 = alloc::zmalloc_socket::<Test>(0);
        assert_eq!(t4.x, 0);
        assert_eq!(t4.y, 0);

        #[allow(unsafe_code)]
        unsafe {
            alloc::free(t1);
            alloc::free(t2);
            alloc::free(t3);
            alloc::free(t4);
        }
    }
}
