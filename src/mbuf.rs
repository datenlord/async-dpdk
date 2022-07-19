//! mbuf

use dpdk_sys::*;
use std::{
    mem::MaybeUninit,
    os::raw::c_void,
    ptr::{self, NonNull},
    slice,
};

use crate::mempool::{Mempool, MempoolObj};

/// The mbuf library provides the ability to allocate and free buffers (mbufs) that may be
/// used by the DPDK application to store message buffers. The message buffers are stored
/// in a mempool, using the Mempool Library.
#[derive(Debug, Clone)]
#[allow(missing_copy_implementations)]
pub struct Mbuf {
    mb: NonNull<rte_mbuf>,
}

impl MempoolObj for Mbuf {
    fn as_c_void(&self) -> *mut c_void {
        self.as_ptr() as *mut c_void
    }
}

#[allow(unsafe_code)]
impl Mbuf {
    fn new(ptr: *mut rte_mbuf) -> Self {
        let mb = NonNull::new(ptr).unwrap();
        Self { mb }
    }
    /// Allocate an uninitialized mbuf from mempool.
    #[inline]
    pub fn alloc(mp: &Mempool) -> Self {
        let ptr = unsafe { rte_pktmbuf_alloc(mp.as_ptr()) };
        Self::new(ptr)
    }

    /// Allocate a bulk of mbufs, initialize refcnt and reset the fields to default values.
    pub fn alloc_bulk(mp: &Mempool, n: u32) -> Vec<Self> {
        let mut mbufs = (0..n)
            .map(|_| MaybeUninit::<rte_mbuf>::uninit())
            .collect::<Vec<_>>();
        let mut ptrs = mbufs
            .iter_mut()
            .map(|mbuf| mbuf.as_mut_ptr())
            .collect::<Vec<_>>();
        let _ = unsafe { rte_pktmbuf_alloc_bulk(mp.as_ptr(), ptrs.as_mut_ptr(), n) };
        mbufs
            .into_iter()
            .map(|mbuf| {
                let mut ptr = unsafe { mbuf.assume_init() };
                Self::new(std::ptr::addr_of_mut!(ptr))
            })
            .collect()
    }

    /// A packet mbuf pool constructor.
    ///
    /// This function initializes the mempool private data in the case of a pktmbuf pool. This
    /// private data is needed by the driver. The function must be called on the mempool before
    /// it is used, or it can be given as a callback function to rte_mempool_create() at pool
    /// creation.
    pub fn init_mp(mp: &Mempool) {
        unsafe {
            rte_pktmbuf_pool_init(mp.as_ptr(), ptr::null_mut());
        }
    }

    /// Return the mbuf owning the data buffer address of an indirect mbuf.
    pub fn from_indirect(mi: &Mbuf) -> Self {
        let ptr = unsafe { rte_mbuf_from_indirect(mi.as_ptr()) };
        Self::new(ptr)
    }

    /// Get the headroom in a packet mbuf.
    #[inline]
    pub fn headroom(&self) -> u32 {
        unsafe { rte_pktmbuf_headroom(self.as_ptr()) as u32 }
    }

    /// Get the tailroom of a packet mbuf.
    #[inline]
    pub fn tailroom(&self) -> u32 {
        unsafe { rte_pktmbuf_tailroom(self.as_ptr()) as u32 }
    }

    /// Create a "clone" of the given packet mbuf.
    ///
    /// Walks through all segments of the given packet mbuf, and for each of them:
    ///  - Creates a new packet mbuf from the given pool.
    ///  - Attaches newly created mbuf to the segment. Then updates pkt_len and nb_segs
    ///    of the "clone" packet mbuf to match values from the original packet mbuf.
    pub fn pktmbuf_clone(&self, mp: &Mempool) -> Self {
        let ptr = unsafe { rte_pktmbuf_clone(self.as_ptr(), mp.as_ptr()) };
        Self::new(ptr)
    }

    /// Create a full copy of a given packet mbuf.
    ///
    /// Copies all the data from a given packet mbuf to a newly allocated set of mbufs.
    /// The private data are is not copied.
    pub fn pktmbuf_copy(&self, mp: &Mempool, offset: u32, length: u32) -> Self {
        let ptr = unsafe { rte_pktmbuf_copy(self.as_ptr(), mp.as_ptr(), offset, length) };
        Self::new(ptr)
    }

    /// Whether this mbuf is indirect
    #[inline]
    pub fn is_indirect(&self) -> bool {
        unsafe { (*self.as_ptr()).ol_flags & RTE_MBUF_F_INDIRECT != 0 }
    }

    /// Test if mbuf data is contiguous (i.e. with only one segment).
    #[inline]
    pub fn is_contiguous(&self) -> bool {
        unsafe { (*self.as_ptr()).nb_segs == 1 }
    }

    /// Return address of buffer embedded in the given mbuf.
    ///
    /// The return value shall be same as mb->buf_addr if the mbuf is already initialized
    /// and direct. However, this API is useful if mempool of the mbuf is already known
    /// because it doesn't need to access mbuf contents in order to get the mempool pointer.
    #[inline]
    pub fn buf_ptr(&self) -> *const u8 {
        let m = self.as_ptr();
        unsafe { rte_mbuf_buf_addr(m, (*m).pool) as *const u8 }
    }

    /// Return address of buffer embedded in the given mbuf.
    #[inline]
    pub fn buf_ptr_mut(&mut self) -> *mut u8 {
        let m = self.as_ptr();
        unsafe { rte_mbuf_buf_addr(m, (*m).pool) as *mut u8 }
    }

    /// Attach packet mbuf to another packet mbuf, thus become an indirect mbuf.
    #[inline]
    pub fn attach(&mut self, mb: &Mbuf) {
        unsafe { rte_pktmbuf_attach(self.as_ptr(), mb.as_ptr()) };
    }

    /// Detach a packet mbuf from external buffer or direct buffer.
    #[inline]
    pub fn detach(&mut self) {
        unsafe { rte_pktmbuf_detach(self.as_ptr()) };
    }

    /// Prepend len bytes to an mbuf data area.
    ///
    /// Returns a pointer to the new data start address.
    /// If there is not enough headroom in the first segment, the function will return NULL,
    /// without modifying the mbuf.
    pub fn prepend(&mut self, len: usize) -> &mut [u8] {
        unsafe {
            let data = rte_pktmbuf_prepend(self.as_ptr(), len as _) as *mut u8;
            slice::from_raw_parts_mut(data, len)
        }
    }

    /// Append len bytes to an mbuf.
    ///
    /// Append len bytes to an mbuf and return a pointer to the start address of the added
    /// data. If there is not enough tailroom in the last segment, the function will return
    /// NULL, without modifying the mbuf.
    pub fn append(&mut self, len: usize) -> &mut [u8] {
        unsafe {
            let data = rte_pktmbuf_append(self.as_ptr(), len as _) as *mut u8;
            slice::from_raw_parts_mut(data, len)
        }
    }

    /// Remove len bytes at the beginning of an mbuf.
    ///
    /// Returns a pointer to the start address of the new data area. If the length is greater
    /// than the length of the first segment, then the function will fail and return NULL,
    /// without modifying the mbuf.
    pub fn adj(&mut self, len: usize) -> &mut [u8] {
        unsafe {
            let data = rte_pktmbuf_adj(self.as_ptr(), len as _) as *mut u8;
            slice::from_raw_parts_mut(data, len)
        }
    }

    /// Remove len bytes of data at the end of the mbuf.
    ///
    /// If the length is greater than the length of the last segment, the function will fail
    /// and return -1 without modifying the mbuf.
    pub fn trim(&mut self, len: usize) -> i32 {
        unsafe { rte_pktmbuf_trim(self.as_ptr(), len as _) }
    }

    /// Chain an mbuf to another, thereby creating a segmented packet.
    ///
    /// Note: The implementation will do a linear walk over the segments to find the tail entry.
    /// For cases when there are many segments, it's better to chain the entries manually.
    pub fn chain_to(&self, tail: &mut Mbuf) {
        let _ = unsafe { rte_pktmbuf_chain(self.as_ptr(), tail.as_ptr()) };
    }

    /// Linearize data in mbuf.
    ///
    /// This function moves the mbuf data in the first segment if there is enough tailroom.
    /// The subsequent segments are unchained and freed.
    pub fn linearize(&mut self) {
        let _ = unsafe { rte_pktmbuf_linearize(self.as_ptr()) };
    }

    pub(crate) fn as_ptr(&self) -> *mut rte_mbuf {
        self.mb.as_ptr()
    }
}

#[allow(unsafe_code)]
impl Drop for Mbuf {
    fn drop(&mut self) {
        unsafe { rte_pktmbuf_free(self.as_ptr()) };
    }
}
