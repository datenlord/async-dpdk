//! Allocator

use dpdk_sys::{rte_free, rte_malloc, rte_malloc_socket, rte_zmalloc, rte_zmalloc_socket};
use std::{mem, ptr, slice};

/// Get a DPDK-allocated immutable slice.
#[inline]
#[must_use]
pub fn slice(size: usize) -> &'static [u8] {
    // SAFETY: ffi
    #[allow(unsafe_code)]
    unsafe {
        let ptr = rte_malloc(ptr::null(), size, 0) as *const u8;
        slice::from_raw_parts(ptr, size)
    }
}

/// Get a DPDK-allocated mutable slice.
#[inline]
#[must_use]
pub fn slice_mut(size: usize) -> &'static mut [u8] {
    // SAFETY: ffi
    #[allow(unsafe_code)]
    unsafe {
        let ptr = rte_malloc(ptr::null(), size, 0).cast::<u8>();
        slice::from_raw_parts_mut(ptr, size)
    }
}

/// This function allocates memory from the huge-page area of memory. The memory is not cleared.
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

/// Allocate zeroed memory from the heap. Equivalent to `rte_malloc`() except that the memory zone is
/// initialised with zeros. In NUMA systems, the memory allocated resides on the same NUMA socket
/// as the core that calls this function.
#[inline]
#[must_use]
pub fn zmalloc<T: Default>() -> Box<T> {
    // SAFETY: ffi
    #[allow(unsafe_code)]
    unsafe {
        let ptr = rte_zmalloc(ptr::null(), mem::size_of::<T>(), 0);
        *ptr.cast::<T>() = T::default();
        Box::from_raw(ptr.cast())
    }
}

/// Malloc on specific socket.
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

/// Zmalloc on specific socket.
#[inline]
#[must_use]
pub fn zmalloc_socket<T: Default>(socket: i32) -> Box<T> {
    // SAFETY: ffi
    #[allow(unsafe_code)]
    unsafe {
        let ptr = rte_zmalloc_socket(ptr::null(), mem::size_of::<T>(), 0, socket);
        *ptr.cast::<T>() = T::default();
        Box::from_raw(ptr.cast())
    }
}

/// Frees the memory space pointed to by the provided pointer. This pointer must have been returned
/// by a previous call to `rte_malloc`(), `rte_zmalloc`(), `rte_calloc`() or `rte_realloc`(). The behaviour of
/// `rte_free`() is undefined if the pointer does not match this requirement.
///
/// If the pointer is NULL, the function does nothing.
#[inline]
pub fn free<T>(obj: Box<T>) {
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
    use crate::eal::{self, IovaMode};

    #[test]
    fn test() {
        #[derive(Default)]
        struct Test {
            x: i32,
            y: i64,
        }

        eal::Config::new().iova_mode(IovaMode::VA).enter().unwrap();

        let t = alloc::malloc::<Test>();
        assert_eq!(t.x, 0);
        assert_eq!(t.y, 0);

        alloc::free(t);
    }
}
