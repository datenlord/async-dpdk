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

use crate::{Error, Result};
use dpdk_sys::{rte_free, rte_malloc, rte_malloc_socket, rte_zmalloc, rte_zmalloc_socket};
use std::{mem, ptr};

/// Check the size of `T` is non-zero, which is required in rte malloc functions.
macro_rules! check_size {
    ($t: ty) => {
        if std::mem::size_of::<$t>() == 0 {
            return Err(crate::Error::InvalidArg);
        }
    };
}

/// This function allocates memory from the huge-page area of memory. The memory is not initialized.
/// In NUMA systems, the memory allocated resides on the same NUMA socket as the core that calls this
/// function.
///
/// # Errors
///
/// - An `Error::NoMem` could be returned if there's no enough memory.
/// - An `Error::InvalidArg` could be returned if the size of `T` is 0.
#[inline]
pub fn malloc<T: Default>() -> Result<Box<T>> {
    check_size!(T);
    // SAFETY: setting `align` to 0 makes sure the return is a pointer that is suitably aligned
    // for any kind of variable (in the same manner as `malloc()`). size checked.
    #[allow(unsafe_code)]
    let ptr = unsafe { rte_malloc(ptr::null(), mem::size_of::<T>(), 0) };
    if ptr.is_null() {
        return Err(Error::NoMem);
    }
    // SAFETY: pointer checked then initialized using `T::default`.
    #[allow(unsafe_code)]
    unsafe {
        *ptr.cast::<T>() = T::default();
        Ok(Box::from_raw(ptr.cast()))
    }
}

/// Allocate zeroed memory from the heap. In NUMA systems, the memory allocated resides on the same NUMA socket
/// as the core that calls this function.
///
/// # Errors
///
/// - An `Error::NoMem` could be returned if there's no enough memory.
/// - An `Error::InvalidArg` could be returned if the size of `T` is 0.
#[inline]
pub fn zmalloc<T>() -> Result<Box<T>> {
    check_size!(T);
    // SAFETY: setting alignment to 0 makes sure the pointer is suitably aligned. size checked.
    #[allow(unsafe_code)]
    let ptr = unsafe { rte_zmalloc(ptr::null(), mem::size_of::<T>(), 0) };
    if ptr.is_null() {
        return Err(Error::NoMem);
    }
    // SAFETY: pointer checked
    #[allow(unsafe_code)]
    unsafe {
        Ok(Box::from_raw(ptr.cast()))
    }
}

/// Allocate memory from hugepages on specific socket.
///
/// # Errors
///
/// - An `Error::NoMem` could be returned if there's no enough memory.
/// - An `Error::InvalidArg` could be returned if the size of `T` is 0.
#[inline]
pub fn malloc_socket<T: Default>(socket: i32) -> Result<Box<T>> {
    check_size!(T);
    // SAFETY: setting `align` to 0 makes sure the pointer is properly aligned. size checked.
    #[allow(unsafe_code)]
    let ptr = unsafe { rte_malloc_socket(ptr::null(), mem::size_of::<T>(), 0, socket) };
    if ptr.is_null() {
        return Err(Error::NoMem);
    }
    // SAFETY: pointer checked and initialized with `T::default`.
    #[allow(unsafe_code)]
    unsafe {
        *ptr.cast::<T>() = T::default();
        Ok(Box::from_raw(ptr.cast()))
    }
}

/// Allocate zeroed memory from hugepages on specific socket.
///
/// # Errors
///
/// - An `Error::NoMem` could be returned if there's no enough memory.
/// - An `Error::InvalidArg` could be returned if the size of `T` is 0.
#[inline]
pub fn zmalloc_socket<T>(socket: i32) -> Result<Box<T>> {
    check_size!(T);
    // SAFETY: setting `align` to 0 makes sure the pointer is properly aligned. size checked.
    #[allow(unsafe_code)]
    let ptr = unsafe { rte_zmalloc_socket(ptr::null(), mem::size_of::<T>(), 0, socket) };
    if ptr.is_null() {
        return Err(Error::NoMem);
    }
    // SAFETY: pointer checked
    #[allow(unsafe_code)]
    unsafe {
        Ok(Box::from_raw(ptr.cast()))
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
    // SAFETY: user should be responsible for the validity of the object pointer.
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

        let t1 = alloc::malloc::<Test>().unwrap();
        assert_eq!(t1.x, 1);
        assert_eq!(t1.y, 2);

        let t2 = alloc::zmalloc::<Test>().unwrap();
        assert_eq!(t2.x, 0);
        assert_eq!(t2.y, 0);

        let t3 = alloc::malloc_socket::<Test>(0).unwrap();
        assert_eq!(t3.x, 1);
        assert_eq!(t3.y, 2);

        let t4 = alloc::zmalloc_socket::<Test>(0).unwrap();
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
