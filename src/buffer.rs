//! txbuffer
use crate::{Error, Result};
use dpdk_sys::*;
use std::{
    ffi::{c_void, CString},
    mem,
    ptr::NonNull,
};

/// Buffer packets which will be sent in the future
#[allow(missing_copy_implementations)]
#[derive(Debug)]
pub struct TxBuffer {
    tb: NonNull<rte_eth_dev_tx_buffer>,
}

#[allow(unsafe_code)]
impl TxBuffer {
    /// Allocate a TxBuffer. Then initialize default values for buffered transmitting.
    pub fn new(size: u16) -> Result<Self> {
        let ty = CString::new("tx_buffer").unwrap();
        // SAFETY: ffi
        let ptr = unsafe {
            rte_zmalloc(ty.as_ptr(), mem::size_of::<rte_eth_dev_tx_buffer>(), 0)
                as *mut rte_eth_dev_tx_buffer
        };
        let errno = unsafe { rte_eth_tx_buffer_init(ptr, size) };
        Error::from_ret(errno)?;
        NonNull::new(ptr).map_or_else(|| Err(Error::from_errno()), |tb| Ok(Self { tb }))
    }

    /// Allocate a TxBuffer on the given socket.
    pub fn new_socket(socket: i32, size: u16) -> Result<Self> {
        let ty = CString::new("tx_buffer").unwrap();
        // SAFETY: ffi
        let ptr = unsafe {
            rte_zmalloc_socket(
                ty.as_ptr(),
                mem::size_of::<rte_eth_dev_tx_buffer>(),
                0,
                socket,
            ) as *mut rte_eth_dev_tx_buffer
        };
        let errno = unsafe { rte_eth_tx_buffer_init(ptr, size) };
        Error::from_ret(errno)?;
        NonNull::new(ptr).map_or_else(|| Err(Error::from_errno()), |tb| Ok(Self { tb }))
    }

    #[inline(always)]
    pub(crate) fn as_ptr(&self) -> *mut rte_eth_dev_tx_buffer {
        self.tb.as_ptr()
    }
}

#[allow(unsafe_code)]
unsafe impl Send for TxBuffer {}

#[allow(unsafe_code)]
unsafe impl Sync for TxBuffer {}

impl Drop for TxBuffer {
    fn drop(&mut self) {
        // SAFETY: ffi
        #[allow(unsafe_code)]
        unsafe {
            rte_free(self.as_ptr() as *mut c_void);
        }
    }
}
