//! mbuf

use dpdk_sys::*;
use std::{ffi::CString, mem::MaybeUninit, ptr::NonNull, slice};

use crate::mempool::{Mempool, MempoolInner};
use crate::{Error, Result};

/// The mbuf library provides the ability to allocate and free buffers (mbufs) that may be
/// used by the DPDK application to store message buffers. The message buffers are stored
/// in a mempool, using the Mempool Library.
///
/// It looks like this:
///     -------------------------------------------------------------------------------
///     | rte_mbuf | priv data | head room |          frame data          | tail room |
///     -------------------------------------------------------------------------------
///     ^                      ^
///  *rte_mbuf              buf_addr
///                             <-data_off-><----------data_len----------->
///                             <------------------------buf_len---------------------->
///
#[derive(Debug, Clone)]
#[allow(missing_copy_implementations)]
pub struct Mbuf {
    mb: NonNull<rte_mbuf>,
}

#[allow(unsafe_code)]
impl Mbuf {
    fn new_with_ptr(ptr: *mut rte_mbuf) -> Result<Self> {
        NonNull::new(ptr).map_or_else(|| Err(Error::from_errno()), |mb| Ok(Self { mb }))
    }

    /// Allocate an uninitialized mbuf from mempool.
    #[inline]
    pub fn new(mp: &Mempool) -> Result<Self> {
        // SAFETY: ffi
        let ptr = unsafe { rte_pktmbuf_alloc(mp.as_ptr()) };
        Self::new_with_ptr(ptr)
    }

    /// Allocate a bulk of mbufs, initialize refcnt and reset the fields to default values.
    pub fn new_bulk(mp: &Mempool, n: u32) -> Result<Vec<Self>> {
        let mut mbufs = (0..n)
            .map(|_| MaybeUninit::<rte_mbuf>::uninit())
            .collect::<Vec<_>>();
        let mut ptrs = mbufs
            .iter_mut()
            .map(|mbuf| mbuf.as_mut_ptr())
            .collect::<Vec<_>>();
        // SAFETY: ffi
        let errno = unsafe { rte_pktmbuf_alloc_bulk(mp.as_ptr(), ptrs.as_mut_ptr(), n) };
        Error::from_ret(errno)?;
        let mut v = vec![];
        for mut mbuf in mbufs {
            let ptr = mbuf.as_mut_ptr();
            // SAFETY: mbufs initialized in `rte_pktmbuf_alloc_bulk`
            let _mbuf = unsafe { mbuf.assume_init() }; // XXX
            v.push(Self::new_with_ptr(ptr)?)
        }
        Ok(v)
    }

    /// Create a mbuf pool.
    ///
    /// This function creates and initializes a packet mbuf pool.
    pub fn create_mp(n: u32, cache_size: u32, socket_id: i32) -> Result<Mempool> {
        let name = CString::new("mbuf pool").unwrap();
        // SAFETY: ffi
        let ptr = unsafe {
            rte_pktmbuf_pool_create(
                name.as_ptr(),
                n,
                cache_size,
                0,
                RTE_MBUF_DEFAULT_BUF_SIZE as u16,
                socket_id,
            )
        };
        let inner = MempoolInner::new(ptr)?;
        Ok(Mempool::new(inner))
    }

    /// Return the mbuf owning the data buffer address of an indirect mbuf.
    pub fn from_indirect(mi: &Mbuf) -> Result<Self> {
        // SAFETY: ffi
        let ptr = unsafe { rte_mbuf_from_indirect(mi.as_ptr()) };
        Self::new_with_ptr(ptr)
    }

    /// Get the data length of a mbuf.
    pub fn data_len(&self) -> u32 {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { (*self.as_ptr()).data_len as u32 }
    }

    /// Get the packet length of a mbuf.
    pub fn pkt_len(&self) -> u32 {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { (*self.as_ptr()).pkt_len }
    }

    /// Get the number of segments of a mbuf.
    pub fn num_segs(&self) -> u32 {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { (*self.as_ptr()).nb_segs as u32 }
    }

    /// Get the headroom in a packet mbuf.
    #[inline]
    pub fn headroom(&self) -> u32 {
        // SAFETY: ffi
        unsafe { rte_pktmbuf_headroom(self.as_ptr()) as u32 }
    }

    /// Get the tailroom of a packet mbuf.
    #[inline]
    pub fn tailroom(&self) -> u32 {
        // SAFETY: ffi
        unsafe { rte_pktmbuf_tailroom(self.as_ptr()) as u32 }
    }

    /// Create a "clone" of the given packet mbuf.
    ///
    /// Walks through all segments of the given packet mbuf, and for each of them:
    ///  - Creates a new packet mbuf from the given pool.
    ///  - Attaches newly created mbuf to the segment. Then updates pkt_len and nb_segs
    ///    of the "clone" packet mbuf to match values from the original packet mbuf.
    pub fn pktmbuf_clone(&self, mp: &Mempool) -> Result<Self> {
        // SAFETY: ffi
        let ptr = unsafe { rte_pktmbuf_clone(self.as_ptr(), mp.as_ptr()) };
        Self::new_with_ptr(ptr)
    }

    /// Create a full copy of a given packet mbuf.
    ///
    /// Copies all the data from a given packet mbuf to a newly allocated set of mbufs.
    /// The private data are is not copied.
    pub fn pktmbuf_copy(&self, mp: &Mempool, offset: u32, length: u32) -> Result<Self> {
        // SAFETY: ffi
        let ptr = unsafe { rte_pktmbuf_copy(self.as_ptr(), mp.as_ptr(), offset, length) };
        Self::new_with_ptr(ptr)
    }

    /// Whether this mbuf is indirect
    #[inline]
    pub fn is_indirect(&self) -> bool {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { (*self.as_ptr()).ol_flags & RTE_MBUF_F_INDIRECT != 0 }
    }

    /// Test if mbuf data is contiguous (i.e. with only one segment).
    #[inline]
    pub fn is_contiguous(&self) -> bool {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { (*self.as_ptr()).nb_segs == 1 }
    }

    /// Return address of buffer embedded in the given mbuf.
    ///
    /// The return value shall be same as mb->buf_addr if the mbuf is already initialized
    /// and direct. However, this API is useful if mempool of the mbuf is already known
    /// because it doesn't need to access mbuf contents in order to get the mempool pointer.
    #[inline]
    pub fn data_slice(&self) -> &[u8] {
        let m = self.as_ptr();
        // SAFETY: ffi; memory is initialized and valid
        unsafe {
            let data = rte_mbuf_buf_addr(m, (*m).pool).add((*m).data_off as _);
            slice::from_raw_parts(data as *const u8, (*m).data_len as _)
        }
    }

    /// Return address of buffer embedded in the given mbuf.
    #[inline]
    pub fn data_slice_mut(&mut self) -> &mut [u8] {
        let m = self.as_ptr();
        // SAFETY: ffi; memory is initialized and valid
        unsafe {
            let data = rte_mbuf_buf_addr(m, (*m).pool).add((*m).data_off as _);
            slice::from_raw_parts_mut(data as *mut u8, (*m).data_len as _)
        }
    }

    /// Attach packet mbuf to another packet mbuf, thus become an indirect mbuf.
    #[inline]
    pub fn attach(&mut self, mb: &Mbuf) {
        // SAFETY: ffi
        unsafe { rte_pktmbuf_attach(self.as_ptr(), mb.as_ptr()) };
    }

    /// Detach a packet mbuf from external buffer or direct buffer.
    #[inline]
    pub fn detach(&mut self) {
        // SAFETY: ffi
        unsafe { rte_pktmbuf_detach(self.as_ptr()) };
    }

    /// Prepend len bytes to an mbuf data area.
    ///
    /// Returns a pointer to the new data start address.
    /// If there is not enough headroom in the first segment, the function will return NULL,
    /// without modifying the mbuf.
    pub fn prepend(&mut self, len: usize) -> Result<&mut [u8]> {
        // SAFETY: ffi
        let data = unsafe { rte_pktmbuf_prepend(self.as_ptr(), len as _) as *mut u8 };
        if data.is_null() {
            return Err(Error::InvalidArg);
        }
        // SAFETY: memory is initialzed
        unsafe { Ok(slice::from_raw_parts_mut(data, len)) }
    }

    /// Append len bytes to an mbuf.
    ///
    /// Append len bytes to an mbuf and return a pointer to the start address of the added
    /// data. If there is not enough tailroom in the last segment, the function will return
    /// NULL, without modifying the mbuf.
    pub fn append(&mut self, len: usize) -> Result<&mut [u8]> {
        // SAFETY: ffi
        let data = unsafe { rte_pktmbuf_append(self.as_ptr(), len as _) as *mut u8 };
        if data.is_null() {
            return Err(Error::InvalidArg);
        }
        // SAFETY: memory is initialzed
        unsafe { Ok(slice::from_raw_parts_mut(data, len)) }
    }

    /// Remove len bytes at the beginning of an mbuf.
    ///
    /// Returns a pointer to the start address of the new data area. If the length is greater
    /// than the length of the first segment, then the function will fail and return NULL,
    /// without modifying the mbuf.
    pub fn adj(&mut self, len: usize) -> Result<()> {
        // SAFETY: ffi
        let data = unsafe { rte_pktmbuf_adj(self.as_ptr(), len as _) as *mut u8 };
        if data.is_null() {
            Err(Error::InvalidArg)
        } else {
            Ok(())
        }
    }

    /// Remove len bytes of data at the end of the mbuf.
    ///
    /// If the length is greater than the length of the last segment, the function will fail
    /// and return -1 without modifying the mbuf.
    pub fn trim(&mut self, len: usize) -> Result<()> {
        // SAFETY: ffi
        let res = unsafe { rte_pktmbuf_trim(self.as_ptr(), len as _) };
        if res == 0 {
            Ok(())
        } else {
            Err(Error::InvalidArg)
        }
    }

    /// Chain an mbuf to another, thereby creating a segmented packet.
    ///
    /// Note: The implementation will do a linear walk over the segments to find the tail entry.
    /// For cases when there are many segments, it's better to chain the entries manually.
    pub fn chain(&mut self, tail: Mbuf) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_pktmbuf_chain(self.as_ptr(), tail.as_ptr()) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Linearize data in mbuf.
    ///
    /// This function moves the mbuf data in the first segment if there is enough tailroom.
    /// The subsequent segments are unchained and freed.
    pub fn linearize(&mut self) -> Result<()> {
        // SAFETY: ffi
        let ret = unsafe { rte_pktmbuf_linearize(self.as_ptr()) };
        if ret == -1 {
            return Err(Error::from_errno());
        }
        Ok(())
    }

    pub(crate) fn as_ptr(&self) -> *mut rte_mbuf {
        self.mb.as_ptr()
    }
}

#[allow(unsafe_code)]
impl Drop for Mbuf {
    fn drop(&mut self) {
        // SAFETY: ffi; this pointer is valid
        unsafe { rte_pktmbuf_free(self.as_ptr()) };
    }
}