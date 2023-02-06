//! The mbuf library provides the ability to allocate and free buffers (mbufs) that may be used
//! by the DPDK application to store message buffers. The message buffers are stored in a mempool,
//! using the Mempool Library.

use crate::mempool::{MempoolObj, PktMempool};
use crate::{Error, Result};
use dpdk_sys::{
    rte_mbuf, rte_mbuf_buf_addr, rte_pktmbuf_adj, rte_pktmbuf_alloc, rte_pktmbuf_alloc_bulk,
    rte_pktmbuf_append, rte_pktmbuf_chain, rte_pktmbuf_clone, rte_pktmbuf_free,
    rte_pktmbuf_headroom, rte_pktmbuf_prepend, rte_pktmbuf_tailroom, rte_pktmbuf_trim,
};
use std::mem::size_of;
use std::os::raw::c_void;
use std::{mem::MaybeUninit, ptr::NonNull, slice};

/// `Mbuf` is used to hold network packets, and it also carries some information about protocols,
/// length, etc, for classification. `Mbuf`s are allocated from a `Mempool`.
///
/// `Mbuf` is a safe wrapper of `rte_mbuf`, the original packet carrier in DPDK, whose memory
/// layout is:
///
/// ```
///     ---------------------------------------------------------------------------------
///     |  rte_mbuf  | priv data | head room |          frame data          | tail room |
///     ---------------------------------------------------------------------------------
///     ^                        ^
///     * rte_mbuf               * buf_addr
///                              <-data_off-><-----------data_len----------->
///                              <------------------------buf_len----------------------->
/// ```
///
/// There's a contiguous slice of memory in `rte_mbuf` to hold frame data. To carry packets
/// with arbitrary lengths, `rte_mbuf`s can be chained together as a linked list.
#[derive(Debug)]
#[allow(missing_copy_implementations)]
pub struct Mbuf {
    /// A pointer to `rte_mbuf`.
    mb: NonNull<rte_mbuf>,
}

impl MempoolObj for Mbuf {
    #[inline]
    fn into_raw(self) -> *mut c_void {
        self.mb.as_ptr().cast()
    }
    #[inline]
    fn from_raw(ptr: *mut c_void) -> Result<Self> {
        Self::new_with_ptr(ptr.cast())
    }
    #[inline]
    fn obj_size() -> usize {
        size_of::<rte_mbuf>()
    }
}

#[allow(unsafe_code)]
impl Mbuf {
    /// Get an `Mbuf` instance with pointer to `rte_mbuf`.
    pub(crate) fn new_with_ptr(ptr: *mut rte_mbuf) -> Result<Self> {
        NonNull::new(ptr).map_or(Err(Error::NoMem), |mb| Ok(Self { mb }))
    }

    /// Allocate an uninitialized `Mbuf` from mempool.
    ///
    /// # Errors
    ///
    /// This function returns an error if allocation fails.
    #[inline]
    pub fn new(mp: &PktMempool) -> Result<Self> {
        // SAFETY: ffi
        let ptr = unsafe { rte_pktmbuf_alloc(mp.as_ptr()) };
        Self::new_with_ptr(ptr)
    }

    /// Allocate a bulk of `Mbuf`s, initialize reference count and reset the fields to default values.
    ///
    /// # Errors
    ///
    /// This function returns an error if allocation fails.
    #[inline]
    pub fn new_bulk(mp: &PktMempool, n: u32) -> Result<Vec<Self>> {
        let mut mbufs = (0..n)
            .map(|_| MaybeUninit::<rte_mbuf>::uninit())
            .collect::<Vec<_>>();
        let mut ptrs = mbufs
            .iter_mut()
            .map(std::mem::MaybeUninit::as_mut_ptr)
            .collect::<Vec<_>>();
        // SAFETY: ffi
        let errno = unsafe { rte_pktmbuf_alloc_bulk(mp.as_ptr(), ptrs.as_mut_ptr(), n) };
        Error::from_ret(errno)?;
        let mut v = vec![];
        for mut mbuf in mbufs {
            let ptr = mbuf.as_mut_ptr();
            // SAFETY: mbufs initialized in `rte_pktmbuf_alloc_bulk`
            let _mbuf = unsafe { mbuf.assume_init() };
            v.push(Self::new_with_ptr(ptr)?);
        }
        Ok(v)
    }

    /// Get the data length of an `Mbuf`.
    #[inline]
    #[must_use]
    pub fn data_len(&self) -> usize {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { (*self.as_ptr()).data_len as usize }
    }

    /// Get the packet length of an `Mbuf`.
    #[inline]
    #[must_use]
    pub fn pkt_len(&self) -> usize {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { (*self.as_ptr()).pkt_len as usize }
    }

    /// Get the number of segments of a `Mbuf`.
    #[inline]
    #[must_use]
    pub fn num_segs(&self) -> u32 {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { u32::from((*self.as_ptr()).nb_segs) }
    }

    /// Get the headroom in an `Mbuf`.
    #[inline]
    #[must_use]
    pub fn headroom(&self) -> usize {
        // SAFETY: ffi
        unsafe { rte_pktmbuf_headroom(self.as_ptr()) as usize }
    }

    /// Get the tailroom of an `Mbuf`.
    #[inline]
    #[must_use]
    pub fn tailroom(&self) -> usize {
        // SAFETY: ffi
        unsafe { rte_pktmbuf_tailroom(self.as_ptr()) as usize }
    }

    /// Create a "clone" of the given packet `Mbuf`.
    ///
    /// New `Mbuf`s are allocated from the given `PktMempool`, and populated with the same content
    /// as ths original `Mbuf`.
    ///
    /// # Errors
    ///
    /// This function returns an error if the allocation failed.
    #[inline]
    pub fn clone(&self, mp: &PktMempool) -> Result<Self> {
        // SAFETY: ffi
        let ptr = unsafe { rte_pktmbuf_clone(self.as_ptr(), mp.as_ptr()) };
        Self::new_with_ptr(ptr)
    }

    /// Test if mbuf data is contiguous (i.e. with only one segment).
    #[inline]
    #[must_use]
    pub fn is_contiguous(&self) -> bool {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { (*self.as_ptr()).nb_segs == 1 }
    }

    /// Return immutable reference of the valid content, which starts at `buf_addr + data_off` with the
    /// length of `data_len`, embedded in the given `Mbuf`.
    #[inline]
    #[must_use]
    pub fn data_slice(&self) -> &[u8] {
        let m = self.as_ptr();
        // SAFETY: ffi; memory is initialized and valid
        unsafe {
            let data = rte_mbuf_buf_addr(m, (*m).pool).add((*m).data_off as _);
            slice::from_raw_parts(data, (*m).data_len as _)
        }
    }

    /// Return mutable reference of the valid content embedded in the given `Mbuf`.
    #[inline]
    pub fn data_slice_mut(&mut self) -> &mut [u8] {
        let m = self.as_ptr();
        // SAFETY: ffi; memory is initialized and valid
        unsafe {
            let data = rte_mbuf_buf_addr(m, (*m).pool).add((*m).data_off as _);
            slice::from_raw_parts_mut(data.cast::<u8>(), (*m).data_len as _)
        }
    }

    /// Prepend `len` bytes to an `Mbuf` data area.
    ///
    /// Returns a mutable reference to the start address of the added data.
    ///
    /// # Errors
    ///
    /// If there is not enough headroom in the first segment, the function fail,
    /// without modifying the `Mbuf`.
    #[inline]
    pub fn prepend(&mut self, len: usize) -> Result<&mut [u8]> {
        // SAFETY: ffi
        let data = unsafe {
            rte_pktmbuf_prepend(self.as_ptr(), len.try_into().map_err(Error::from)?).cast::<u8>()
        };
        if data.is_null() {
            return Err(Error::NoMem);
        }
        // SAFETY: memory is valid since data is not null
        unsafe { Ok(slice::from_raw_parts_mut(data, len)) }
    }

    /// Append `len` bytes to an `Mbuf`.
    ///
    /// Return a mutable reference to the start address of the added data.
    ///
    /// # Errors
    ///
    /// If there is not enough tailroom in the last segment, the function will fail,
    /// without modifying the `Mbuf`.
    #[inline]
    pub fn append(&mut self, len: usize) -> Result<&mut [u8]> {
        // SAFETY: ffi
        let data = unsafe {
            rte_pktmbuf_append(self.as_ptr(), len.try_into().map_err(Error::from)?).cast::<u8>()
        };
        if data.is_null() {
            return Err(Error::NoMem);
        }
        // SAFETY: memory is initialzed
        unsafe { Ok(slice::from_raw_parts_mut(data, len)) }
    }

    /// Remove `len` bytes at the beginning of an `Mbuf`. Returns a pointer to the start
    /// address of the new data area.
    ///
    /// # Errors
    ///
    /// If the length is greater than the length of the first segment, then the function
    /// will fail with the `Mbuf` unchanged.
    #[inline]
    pub fn adj(&mut self, len: usize) -> Result<()> {
        // SAFETY: ffi
        let data = unsafe {
            rte_pktmbuf_adj(self.as_ptr(), len.try_into().map_err(Error::from)?).cast::<u8>()
        };
        if data.is_null() {
            Err(Error::InvalidArg)
        } else {
            Ok(())
        }
    }

    /// Remove `len` bytes of data at the end of the `Mbuf`.
    ///
    /// # Errors
    ///
    /// If the length is greater than the length of the last segment, the function will fail
    /// with the `Mbuf` unchanged.
    #[inline]
    pub fn trim(&mut self, len: usize) -> Result<()> {
        // SAFETY: ffi
        let res = unsafe { rte_pktmbuf_trim(self.as_ptr(), len.try_into().map_err(Error::from)?) };
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
    ///
    /// # Errors
    ///
    /// The chain segment limit exceeded.
    #[inline]
    #[allow(clippy::needless_pass_by_value)] // the ownership of the last one should be taken
    pub fn chain(&mut self, tail: Mbuf) -> std::result::Result<(), (Error, Mbuf)> {
        // SAFETY: ffi
        let errno = unsafe { rte_pktmbuf_chain(self.as_ptr(), tail.as_ptr()) };
        if let Err(err) = Error::from_ret(errno) {
            return Err((err, tail));
        }
        Ok(())
    }

    /// Get the next `Mbuf` chained.
    #[must_use]
    #[inline]
    pub fn next(&self) -> Option<Mbuf> {
        // SAFETY: self is valid since it's tested when initialized.
        let ptr = unsafe { (*self.as_ptr()).next };
        let m = Mbuf::new_with_ptr(ptr).ok()?;
        Some(m)
    }

    /// Get pointer to `rte_mbuf`.
    pub(crate) fn as_ptr(&self) -> *mut rte_mbuf {
        self.mb.as_ptr()
    }
}

// SAFETY: nothing thread-local involved.
#[allow(unsafe_code)]
unsafe impl Send for Mbuf {}

impl Drop for Mbuf {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: ffi; this pointer is valid
        #[allow(unsafe_code)]
        unsafe {
            rte_pktmbuf_free(self.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::eal::{self, IovaMode};
    use crate::mbuf::Mbuf;
    use crate::mempool::{Mempool, PktMempool};

    #[test]
    fn test() {
        let _ = eal::Config::new().iova_mode(IovaMode::VA).enter();

        // Create a packet mempool.
        let mp = PktMempool::create("test", 10).unwrap();
        let mut mbuf = Mbuf::new(&mp).unwrap();
        assert!(mbuf.is_contiguous());
        assert_eq!(mbuf.data_len(), 0);
        assert_eq!(mbuf.num_segs(), 1);
        assert_eq!(mbuf.headroom(), 0);
        assert_eq!(mbuf.tailroom(), 2176);

        // Read and write from mbuf.
        let data = mbuf.append(10).unwrap();
        data.copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(mbuf.data_len(), 10);
        assert_eq!(mbuf.data_slice(), &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);

        let data = mbuf.prepend(5).unwrap();
        data.copy_from_slice(&[4, 3, 2, 1, 0]);
        assert_eq!(mbuf.data_len(), 15);
        assert_eq!(
            mbuf.data_slice(),
            &[4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]
        );

        mbuf.adj(5).unwrap();
        assert_eq!(mbuf.data_len(), 10);
        assert_eq!(mbuf.data_slice(), &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);

        mbuf.trim(5).unwrap();
        assert_eq!(mbuf.data_len(), 5);
        assert_eq!(mbuf.data_slice(), &[0, 1, 2, 3, 4]);

        // Mbuf chaining.
        let mbufs = Mbuf::new_bulk(&mp, 2).unwrap();
        for mut m in mbufs.into_iter() {
            let _ = m.append(5).unwrap();
            mbuf.chain(m).unwrap();
        }
        assert_eq!(mbuf.num_segs(), 3);
        assert_eq!(mbuf.pkt_len(), 15);

        // Indirect mbuf.
        let mbuf2 = mbuf.clone(&mp).unwrap();
        assert_eq!(mbuf2.data_slice(), &[0, 1, 2, 3, 4]);
        assert_eq!(mbuf2.num_segs(), 3);
    }
}
