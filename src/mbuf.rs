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
use std::{
    marker::PhantomData,
    mem::{self, size_of, ManuallyDrop},
    ops::{Deref, DerefMut},
    os::raw::c_void,
    ptr::{self, NonNull},
    result::Result as StdResult,
    slice,
};

/// `Mbuf` is used to hold network packets.
///
/// It also carries some information about protocols, length, etc, for packet classification.
/// `Mbuf`s are allocated from a `Mempool`.
///
/// `Mbuf` is a safe wrapper of `rte_mbuf`, the original packet carrier in DPDK, whose memory
/// layout is:
///
/// ```ignore
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
    /// Get an `Mbuf` instance with pointer to `rte_mbuf`. Pointer is checked to
    /// be non-null here.
    pub(crate) fn new_with_ptr(ptr: *mut rte_mbuf) -> Result<Self> {
        NonNull::new(ptr).map_or(Err(Error::NoMem), |mb| Ok(Self { mb }))
    }

    /// Allocate a new `Mbuf` instance from the given `PktMempool`.
    ///
    /// # Errors
    ///
    /// This function returns an error if allocation fails.
    #[inline]
    pub fn new(mp: &PktMempool) -> Result<Self> {
        // SAFETY: check pointer in `Self::new_with_ptr`. Fields in `rte_mbuf` are set
        // to default values. DPDK allocated objects are aligned to the cacheline size.
        let ptr = unsafe { rte_pktmbuf_alloc(mp.as_ptr()) };
        Self::new_with_ptr(ptr)
    }

    /// Allocate a bulk of `Mbuf`s from the given `PktMempool`.
    ///
    /// # Errors
    ///
    /// This function returns an error if allocation fails.
    #[inline]
    pub fn new_bulk(mp: &PktMempool, n: u32) -> Result<Vec<Self>> {
        let mut ptrs = (0..n).map(|_| ptr::null_mut()).collect::<Vec<_>>();
        // SAFETY: invalid allocation result in a negative errno, which is checked later.
        // In this function, fields are set to default values.
        let errno = unsafe { rte_pktmbuf_alloc_bulk(mp.as_ptr(), ptrs.as_mut_ptr(), n) };
        Error::from_ret(errno)?;
        let mut v = vec![];
        for ptr in ptrs {
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

    /// Get the number of segments of an `Mbuf`.
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
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { rte_pktmbuf_headroom(self.as_ptr()) as usize }
    }

    /// Get the tailroom of an `Mbuf`.
    #[inline]
    #[must_use]
    pub fn tailroom(&self) -> usize {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
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
        // SAFETY: pointer checked in `Self::new_with_ptr`
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

    /// Return an immutable reference of the valid content, which starts at `buf_addr + data_off`
    /// with the length of `data_len`, embedded in the given `Mbuf`.
    #[inline]
    #[must_use]
    pub fn data_slice(&self) -> &[u8] {
        let m = self.as_ptr();
        // SAFETY: memory is initialized and valid
        unsafe {
            let data = rte_mbuf_buf_addr(m, (*m).pool).add((*m).data_off as _);
            slice::from_raw_parts(data.cast::<u8>(), (*m).data_len as _)
        }
    }

    /// Return a mutable reference of the valid content embedded in the given `Mbuf`.
    #[inline]
    pub fn data_slice_mut(&mut self) -> &mut [u8] {
        let m = self.as_ptr();
        // SAFETY: memory is initialized and valid
        unsafe {
            let data = rte_mbuf_buf_addr(m, (*m).pool).add((*m).data_off as _);
            slice::from_raw_parts_mut(data.cast::<u8>(), (*m).data_len as _)
        }
    }

    /// Prepend `len` bytes to an `Mbuf` data area.
    ///
    /// Returns a mutable reference to the appended area.
    ///
    /// # Errors
    ///
    /// If there is not enough headroom in the first segment, the function fails,
    /// without modifying the `Mbuf`.
    #[inline]
    pub fn prepend(&mut self, len: usize) -> Result<&mut [u8]> {
        // SAFETY: returned pointer checked later
        let data = unsafe {
            rte_pktmbuf_prepend(self.as_ptr(), len.try_into().map_err(Error::from)?).cast::<u8>()
        };
        if data.is_null() {
            return Err(Error::NoMem);
        }
        // SAFETY: memory is valid
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
        // SAFETY: returned pointer checked later
        let data = unsafe {
            rte_pktmbuf_append(self.as_ptr(), len.try_into().map_err(Error::from)?).cast::<u8>()
        };
        if data.is_null() {
            return Err(Error::NoMem);
        }
        // SAFETY: memory is valid
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
        // SAFETY: returned pointer checked later
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
        // SAFETY: *rte_mbuf pointer checked
        let res = unsafe { rte_pktmbuf_trim(self.as_ptr(), len.try_into().map_err(Error::from)?) };
        if res == 0 {
            Ok(())
        } else {
            Err(Error::InvalidArg)
        }
    }

    /// Pop the mbuf at the front.
    #[inline]
    #[must_use]
    pub fn pop_mbuf(self) -> Option<Mbuf> {
        let prev = self.as_ptr();
        // SAFETY: `prev` checked in `Mbuf::new`
        let ptr = unsafe {
            let ptr = (*prev).next;
            (*prev).next = ptr::null_mut();
            ptr
        };
        let m = Mbuf::new_with_ptr(ptr).ok()?;
        // SAFETY: `ptr` and `prev` checked
        unsafe {
            (*ptr).refcnt = (*prev).refcnt;
            (*ptr).nb_segs = (*prev).nb_segs.saturating_sub(1);
            (*ptr).ol_flags = (*prev).ol_flags;
            (*ptr).pkt_len = (*prev).pkt_len.saturating_sub(u32::from((*prev).data_len));
            (*ptr)
                .packet_type_union
                .clone_from(&(*prev).packet_type_union);
            (*ptr)
                .tx_offload_union
                .clone_from(&(*prev).tx_offload_union);
        }
        Some(m)
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
    pub fn chain_mbuf(&mut self, tail: Mbuf) -> StdResult<(), (Error, Mbuf)> {
        // SAFETY: *rte_mbuf pointers checked
        let errno = unsafe { rte_pktmbuf_chain(self.as_ptr(), tail.as_ptr()) };
        if let Err(err) = Error::from_ret(errno) {
            return Err((err, tail));
        }
        #[allow(clippy::mem_forget)] // deallocated with `head`
        mem::forget(tail);
        Ok(())
    }

    /// Get an immutable iterator of `Mbuf`.
    #[inline]
    #[must_use]
    pub fn iter(&self) -> MbufIter<'_> {
        MbufIter {
            cur: self.as_ptr(),
            _marker: PhantomData,
        }
    }

    /// Get a mutable iterator of `Mbuf`.
    #[inline]
    #[must_use]
    pub fn iter_mut(&mut self) -> MbufIterMut<'_> {
        MbufIterMut {
            cur: self.as_ptr(),
            _marker: PhantomData,
        }
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
        // SAFETY: self pointer checked upon `new`
        #[allow(unsafe_code)]
        unsafe {
            rte_pktmbuf_free(self.as_ptr());
        }
    }
}

/// `Mbuf` immutable iterator.
#[allow(missing_copy_implementations, clippy::module_name_repetitions)]
#[derive(Debug)]
pub struct MbufIter<'a> {
    /// Current `rte_mbuf` pointer.
    cur: *mut rte_mbuf,
    /// Lifetime marker.
    _marker: PhantomData<&'a Mbuf>,
}

impl<'a> Iterator for MbufIter<'a> {
    type Item = MbufRef<'a>;

    #[inline]
    #[allow(unsafe_code)]
    fn next(&mut self) -> Option<Self::Item> {
        let item = MbufRef::new(self.cur)?;
        // SAFETY: `self.cur` checked in `MbufRef::new`
        self.cur = unsafe { (*self.cur).next };
        Some(item)
    }
}

/// `Mbuf` mutable iterator.
#[allow(missing_copy_implementations, clippy::module_name_repetitions)]
#[derive(Debug)]
pub struct MbufIterMut<'a> {
    /// Current `rte_mbuf` pointer.
    cur: *mut rte_mbuf,
    /// Lifetime marker.
    _marker: PhantomData<&'a Mbuf>,
}

impl<'a> Iterator for MbufIterMut<'a> {
    type Item = MbufRefMut<'a>;

    #[inline]
    #[allow(unsafe_code)]
    fn next(&mut self) -> Option<Self::Item> {
        let item = MbufRefMut::new(self.cur)?;
        // SAFETY: `self.cur` checked in `MbufRefMut::new`
        self.cur = unsafe { (*self.cur).next };
        Some(item)
    }
}

/// `Mbuf` immutable reference.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug)]
pub struct MbufRef<'a> {
    /// An `Mbuf` reference.
    mb: ManuallyDrop<Mbuf>,
    /// Lifetime marker.
    _marker: PhantomData<&'a Mbuf>,
}

impl MbufRef<'_> {
    /// Get a new instance of `MbufRef`.
    fn new(ptr: *mut rte_mbuf) -> Option<Self> {
        let mb = Mbuf::new_with_ptr(ptr).ok()?;
        Some(Self {
            mb: ManuallyDrop::new(mb),
            _marker: PhantomData,
        })
    }
}

impl Deref for MbufRef<'_> {
    type Target = Mbuf;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.mb
    }
}

/// `Mbuf` mutable reference.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug)]
pub struct MbufRefMut<'a> {
    /// An `Mbuf` reference.
    mb: ManuallyDrop<Mbuf>,
    /// Lifetime marker.
    _marker: PhantomData<&'a Mbuf>,
}

impl MbufRefMut<'_> {
    /// Get a new instance of `MbufRef`.
    fn new(ptr: *mut rte_mbuf) -> Option<Self> {
        let mb = Mbuf::new_with_ptr(ptr).ok()?;
        Some(Self {
            // to avoid double free
            mb: ManuallyDrop::new(mb),
            _marker: PhantomData,
        })
    }
}

impl Deref for MbufRefMut<'_> {
    type Target = Mbuf;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.mb
    }
}
impl DerefMut for MbufRefMut<'_> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.mb
    }
}

#[cfg(test)]
mod tests {
    use crate::mbuf::Mbuf;
    use crate::mempool::{Mempool, PktMempool};
    use crate::test_utils;

    #[test]
    fn test() {
        test_utils::dpdk_setup();

        // Create a packet mempool.
        let mp = PktMempool::create("test", 10).unwrap();

        let mut mbuf = Mbuf::new(&mp).unwrap();
        assert!(mbuf.is_contiguous());
        assert_eq!(mbuf.data_len(), 0);
        assert_eq!(mbuf.num_segs(), 1);
        assert_eq!(mbuf.headroom(), 128);
        assert_eq!(mbuf.tailroom(), 2048);

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
        let mut mbufs = Mbuf::new_bulk(&mp, 2).unwrap();
        let mut mbuf2 = mbufs.pop().unwrap();
        let mut mbuf1 = mbufs.pop().unwrap();
        let _ = mbuf1.append(5).unwrap();
        let _ = mbuf2.append(5).unwrap();
        mbuf1.chain_mbuf(mbuf2).unwrap();
        mbuf.chain_mbuf(mbuf1).unwrap();
        assert_eq!(mbuf.num_segs(), 3);
        assert_eq!(mbuf.pkt_len(), 15);

        // Mbuf iteration
        for m in mbuf.iter() {
            assert_eq!(m.data_len(), 5);
        }

        // Clone mbuf.
        let mbuf2 = mbuf.clone(&mp).unwrap();
        assert_eq!(mbuf2.data_slice(), &[0, 1, 2, 3, 4]);
        assert_eq!(mbuf2.num_segs(), 3);

        // Mbuf pop
        let mbuf = mbuf.pop_mbuf().unwrap();
        assert_eq!(mbuf.num_segs(), 2);
        assert_eq!(mbuf.pkt_len(), 10);

        // Mbuf iteration
        for m in mbuf.iter() {
            assert_eq!(m.data_len(), 5);
        }
    }
}
