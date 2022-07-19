//! Allocator

use std::mem::MaybeUninit;
// use std::{mem::{self, MaybeUninit}, ffi::CString};

// use dpdk_sys::*;

/// This function allocates memory from the huge-page area of memory.
/// The memory is not cleared.
pub fn alloc<T>() -> MaybeUninit<T> {
    todo!()
    // let mut value = MaybeUninit::<T>::uninit();
    // let ty = CString::new("anon").unwrap();
    // #[allow(unsafe_code)]
    // unsafe {
    //     let ptr = rte_malloc(ty.as_ptr(), mem::size_of::<T>(), 0);
    // };
    // value
}
