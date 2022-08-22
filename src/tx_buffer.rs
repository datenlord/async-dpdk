//! txbuffer
use crate::mbuf::*;
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
    /// Allocate a TxBuffer.
    pub fn alloc() -> Result<Self> {
        let ty = CString::new("tx_buffer").unwrap();
        // SAFETY: ffi
        let ptr = unsafe {
            rte_zmalloc(ty.as_ptr(), mem::size_of::<rte_eth_dev_tx_buffer>(), 0)
                as *mut rte_eth_dev_tx_buffer
        };
        NonNull::new(ptr).map_or_else(|| Err(Error::from_errno()), |tb| Ok(Self { tb }))
    }

    /// Allocate a TxBuffer on the given socket.
    pub fn alloc_socket(socket: i32) -> Result<Self> {
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
        NonNull::new(ptr).map_or_else(|| Err(Error::from_errno()), |tb| Ok(Self { tb }))
    }

    /// Initialize default values for buffered transmitting.
    pub fn init(&mut self, size: u16) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_eth_tx_buffer_init(self.as_ptr(), size) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Send any packets queued up for transmission on a port and HW queue.
    ///
    /// This causes an explicit flush of packets previously buffered via the rte_eth_tx_buffer()
    /// function. It returns the number of packets successfully sent to the NIC, and calls the
    /// error callback for any unsent packets. Unless explicitly set up otherwise, the default
    /// callback simply frees the unsent packets back to the owning mempool.
    pub fn flush(&mut self, port_id: u16, queue_id: u16) -> u16 {
        // SAFETY: ffi
        unsafe { rte_eth_tx_buffer_flush(port_id, queue_id, self.as_ptr()) }
    }

    /// Buffer a single packet for future transmission on a port and queue.
    ///
    /// This function takes a single mbuf/packet and buffers it for later transmission on the
    /// particular port and queue specified. Once the buffer is full of packets, an attempt will
    /// be made to transmit all the buffered packets. In case of error, where not all packets
    /// can be transmitted, a callback is called with the unsent packets as a parameter. If no
    /// callback is explicitly set up, the unsent packets are just freed back to the owning
    /// mempool. The function returns the number of packets actually sent i.e. 0 if no buffer
    /// flush occurred, otherwise the number of packets successfully flushed.
    pub fn buffer(&mut self, pkt: &Mbuf, port_id: u16, queue_id: u16) -> u16 {
        // SAFETY: ffi
        unsafe { rte_eth_tx_buffer(port_id, queue_id, self.as_ptr(), pkt.as_ptr()) }
    }

    #[inline(always)]
    fn as_ptr(&self) -> *mut rte_eth_dev_tx_buffer {
        self.tb.as_ptr()
    }
}

impl Drop for TxBuffer {
    fn drop(&mut self) {
        // SAFETY: ffi
        #[allow(unsafe_code)]
        unsafe {
            rte_free(self.as_ptr() as *mut c_void);
        }
    }
}
