//! mbuf

use crate::mempool::{Mempool, MempoolInner};
use crate::{Error, Result};
use dpdk_sys::{
    rte_mbuf, rte_mbuf_buf_addr, rte_pktmbuf_adj, rte_pktmbuf_alloc, rte_pktmbuf_alloc_bulk,
    rte_pktmbuf_append, rte_pktmbuf_chain, rte_pktmbuf_clone, rte_pktmbuf_free,
    rte_pktmbuf_headroom, rte_pktmbuf_pool_create, rte_pktmbuf_prepend, rte_pktmbuf_tailroom,
    rte_pktmbuf_trim, RTE_MBUF_DEFAULT_BUF_SIZE, RTE_MBUF_F_INDIRECT,
};
use std::ffi::CString;
use std::{mem::MaybeUninit, ptr::NonNull, slice};

/// In this crate we use usize as length for convenience, however DPDK use u16 to represent
/// length, we should check the `len` argument is not too large for u16.
macro_rules! check_len {
    ($len:expr) => {
        if $len > u16::MAX as usize {
            return Err(Error::InvalidArg);
        }
    };
}

///
/// The mbuf library provides the ability to allocate and free buffers (mbufs) that may be
/// used by the DPDK application to store message buffers. The message buffers are stored
/// in a mempool, using the Mempool Library.
///
/// It looks like this:
///     ---------------------------------------------------------------------------------
///     | `rte_mbuf` | priv data | head room |          frame data          | tail room |
///     ---------------------------------------------------------------------------------
///     ^                        ^
///     *`rte_mbuf`              `buf_addr`
///                              <-data_off-><-----------data_len----------->
///                              <------------------------`buf_len`--------------------->
///
#[derive(Debug, Clone)]
#[allow(missing_copy_implementations)]
pub struct Mbuf {
    /// A pointer to `rte_mbuf`.
    mb: NonNull<rte_mbuf>,
}

#[allow(unsafe_code)]
impl Mbuf {
    /// Get a `Mbuf` instance with pointer to `rte_mbuf`.
    pub(crate) fn new_with_ptr(ptr: *mut rte_mbuf) -> Result<Self> {
        NonNull::new(ptr).map_or(Err(Error::NoMem), |mb| Ok(Self { mb }))
    }

    /// Allocate an uninitialized mbuf from mempool.
    #[inline]
    pub fn new(mp: &Mempool) -> Result<Self> {
        // SAFETY: ffi
        let ptr = unsafe { rte_pktmbuf_alloc(mp.as_ptr()) };
        Self::new_with_ptr(ptr)
    }

    /// Allocate a bulk of mbufs, initialize refcnt and reset the fields to default values.
    #[inline]
    pub fn new_bulk(mp: &Mempool, n: u32) -> Result<Vec<Self>> {
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

    /// Create a mbuf pool.
    ///
    /// This function creates and initializes a packet mbuf pool.
    #[inline]
    pub fn create_mp(name: &str, n: u32, cache_size: u32, socket_id: i32) -> Result<Mempool> {
        // SAFETY: ffi
        let ptr = unsafe {
            let name = CString::new(name).map_err(Error::from)?;
            #[allow(clippy::cast_possible_truncation)] // 0x880 < u16::MAX
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

    /// Get the data length of a mbuf.
    #[inline]
    #[must_use]
    pub fn data_len(&self) -> usize {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { (*self.as_ptr()).data_len as usize }
    }

    /// Get the packet length of a mbuf.
    #[inline]
    #[must_use]
    pub fn pkt_len(&self) -> usize {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { (*self.as_ptr()).pkt_len as usize }
    }

    /// Get the number of segments of a mbuf.
    #[inline]
    #[must_use]
    pub fn num_segs(&self) -> u32 {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { u32::from((*self.as_ptr()).nb_segs) }
    }

    /// Get the headroom in a packet mbuf.
    #[inline]
    #[must_use]
    pub fn headroom(&self) -> usize {
        // SAFETY: ffi
        unsafe { rte_pktmbuf_headroom(self.as_ptr()) as usize }
    }

    /// Get the tailroom of a packet mbuf.
    #[inline]
    #[must_use]
    pub fn tailroom(&self) -> usize {
        // SAFETY: ffi
        unsafe { rte_pktmbuf_tailroom(self.as_ptr()) as usize }
    }

    /// Create a "clone" of the given packet mbuf.
    ///
    /// Walks through all segments of the given packet mbuf, and for each of them:
    ///  - Creates a new packet mbuf from the given pool.
    ///  - Attaches newly created mbuf to the segment. Then updates `pkt_len` and `nb_segs`
    ///    of the "clone" packet mbuf to match values from the original packet mbuf.
    #[inline]
    pub fn pktmbuf_clone(&self, mp: &Mempool) -> Result<Self> {
        // SAFETY: ffi
        let ptr = unsafe { rte_pktmbuf_clone(self.as_ptr(), mp.as_ptr()) };
        Self::new_with_ptr(ptr)
    }

    /// Whether this mbuf is indirect
    #[inline]
    #[must_use]
    pub fn is_indirect(&self) -> bool {
        // SAFETY: the *rte_mbuf pointer is checked at initialization and never changes
        unsafe { (*self.as_ptr()).ol_flags & RTE_MBUF_F_INDIRECT != 0 }
    }

    /// Test if mbuf data is contiguous (i.e. with only one segment).
    #[inline]
    #[must_use]
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
    #[must_use]
    pub fn data_slice(&self) -> &[u8] {
        let m = self.as_ptr();
        // SAFETY: ffi; memory is initialized and valid
        unsafe {
            let data = rte_mbuf_buf_addr(m, (*m).pool)
                .add((*m).data_off as _)
                .cast();
            slice::from_raw_parts(data, (*m).data_len as _)
        }
    }

    /// Return address of buffer embedded in the given mbuf.
    #[inline]
    pub fn data_slice_mut(&mut self) -> &mut [u8] {
        let m = self.as_ptr();
        // SAFETY: ffi; memory is initialized and valid
        unsafe {
            let data = rte_mbuf_buf_addr(m, (*m).pool).add((*m).data_off as _);
            slice::from_raw_parts_mut(data.cast::<u8>(), (*m).data_len as _)
        }
    }

    /// Prepend len bytes to an mbuf data area.
    ///
    /// Returns a pointer to the new data start address.
    /// If there is not enough headroom in the first segment, the function will return NULL,
    /// without modifying the mbuf.
    #[inline]
    pub fn prepend(&mut self, len: usize) -> Result<&mut [u8]> {
        check_len!(len);
        // SAFETY: ffi
        let data = unsafe {
            #[allow(clippy::cast_possible_truncation)] // checked
            rte_pktmbuf_prepend(self.as_ptr(), len as _).cast::<u8>()
        };
        if data.is_null() {
            return Err(Error::InvalidArg);
        }
        // SAFETY: memory is valid since data is not null
        unsafe { Ok(slice::from_raw_parts_mut(data, len)) }
    }

    /// Append len bytes to an mbuf.
    ///
    /// Append len bytes to an mbuf and return a pointer to the start address of the added
    /// data. If there is not enough tailroom in the last segment, the function will return
    /// NULL, without modifying the mbuf.
    #[inline]
    pub fn append(&mut self, len: usize) -> Result<&mut [u8]> {
        check_len!(len);
        // SAFETY: ffi
        let data = unsafe {
            #[allow(clippy::cast_possible_truncation)] // checked
            rte_pktmbuf_append(self.as_ptr(), len as _).cast::<u8>()
        };
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
    #[inline]
    pub fn adj(&mut self, len: usize) -> Result<()> {
        check_len!(len);
        // SAFETY: ffi
        let data = unsafe {
            #[allow(clippy::cast_possible_truncation)] // checked
            rte_pktmbuf_adj(self.as_ptr(), len as _).cast::<u8>()
        };
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
    #[inline]
    pub fn trim(&mut self, len: usize) -> Result<()> {
        check_len!(len);
        // SAFETY: ffi
        let res = unsafe {
            #[allow(clippy::cast_possible_truncation)] // checked
            rte_pktmbuf_trim(self.as_ptr(), len as _)
        };
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
    #[inline]
    #[allow(clippy::needless_pass_by_value)] // the ownership of the last one should be taken
    pub fn chain(&mut self, tail: Mbuf) -> Result<()> {
        // SAFETY: ffi
        let errno = unsafe { rte_pktmbuf_chain(self.as_ptr(), tail.as_ptr()) };
        Error::from_ret(errno)?;
        Ok(())
    }

    /// Get the next Mbuf chained.
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
mod test {
    use crate::eal::{self, IovaMode};
    use crate::lcore;
    use crate::mbuf::Mbuf;

    #[test]
    fn test() {
        let _ = eal::Config::new().iova_mode(IovaMode::VA).enter();

        // Create a packet mempool.
        let mp = Mbuf::create_mp("test", 10, 0, lcore::socket_id()).unwrap();
        let mut mbuf = Mbuf::new(&mp).unwrap();
        assert!(mbuf.is_contiguous());
        assert_eq!(mbuf.data_len(), 0);

        // Read and write from mbuf.
        let data = mbuf.append(10).unwrap();
        data.copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(mbuf.data_len(), 10);
        assert_eq!(mbuf.data_slice(), &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
        mbuf.trim(5).unwrap();
        assert_eq!(mbuf.data_len(), 5);
        assert_eq!(mbuf.data_slice(), &[0, 1, 2, 3, 4]);

        // Mbuf chaining.
        let mut mbuf1 = Mbuf::new(&mp).unwrap();
        let _ = mbuf1.append(5).unwrap();
        mbuf.chain(mbuf1).unwrap();
        assert_eq!(mbuf.num_segs(), 2);
        assert_eq!(mbuf.pkt_len(), 10);

        // Indirect mbuf.
        let mbuf2 = mbuf.pktmbuf_clone(&mp).unwrap();
        assert_eq!(mbuf2.data_slice(), &[0, 1, 2, 3, 4]);
    }
}
