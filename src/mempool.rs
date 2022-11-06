//! mempool wrapper

use crate::{Error, Result};
use dpdk_sys::{
    rte_mempool, rte_mempool_avail_count, rte_mempool_create, rte_mempool_free, rte_mempool_get,
    rte_mempool_get_bulk, rte_mempool_in_use_count, rte_mempool_lookup, rte_mempool_put_bulk,
    RTE_MEMPOOL_F_NO_CACHE_ALIGN, RTE_MEMPOOL_F_NO_IOVA_CONTIG, RTE_MEMPOOL_F_NO_SPREAD,
    RTE_MEMPOOL_F_SC_GET, RTE_MEMPOOL_F_SP_PUT,
};
use lazy_static::lazy_static;
use std::collections::HashMap;
use std::ffi::CString;
use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::os::raw::c_void;
use std::ptr::{self, NonNull};
use std::sync::Mutex;
use std::sync::{Arc, Weak};

lazy_static! {
    pub(crate) static ref MEMPOOLS: Mutex<HashMap<usize, Weak<MempoolInner>>> = Mutex::default();
}

/// Mempool flag. By default, objects addresses are spread between channels in RAM: the pool
/// allocator will add padding between objects depending on the hardware configuration. If this flag
/// is set, the allocator will just align them to a cache line.
pub const MEMPOOL_NO_SPREAD: u32 = RTE_MEMPOOL_F_NO_SPREAD;

/// By default, the returned objects are cache-aligned. This flag removes this constraint,
/// and no padding will be present between objects. This flag implies `RTE_MEMPOOL_F_NO_SPREAD`.
pub const MEMPOOL_NO_CACHE_ALIGN: u32 = RTE_MEMPOOL_F_NO_CACHE_ALIGN;

/// If this flag is set, the default behavior when using `rte_mempool_put`() or `rte_mempool_put_bulk`()
/// is "single-producer". Otherwise, it is "multi-producers".
pub const MEMPOOL_SINGLE_PRODUCER: u32 = RTE_MEMPOOL_F_SP_PUT;

/// If this flag is set, the default behavior when using `rte_mempool_get`() or `rte_mempool_get_bulk`()
/// is "single-consumer". Otherwise, it is "multi-consumers".
pub const MEMPOOL_SINGLE_CONSUMER: u32 = RTE_MEMPOOL_F_SC_GET;

/// If set, allocated objects won't necessarily be contiguous in IO memory.
pub const MEMPOOL_NO_IOVA_CONTIG: u32 = RTE_MEMPOOL_F_NO_IOVA_CONTIG;

/// Mempool is an allocator for fixed sized object.In DPDK, it's identified by name and uses a
/// mempool handler to store free objects. It provides some other optional services such as
/// per-core object cache and an alignment helper.
#[derive(Debug)]
pub struct Mempool {
    /// An `Arc` pointer to `MempoolInner`.
    inner: Arc<MempoolInner>,
}

impl Mempool {
    /// Create a new Mempool named `name` in memory.
    #[inline]
    #[allow(clippy::unwrap_in_result)]
    pub fn create(
        name: &str,
        n: u32,
        elt_size: u32,
        cache_size: u32,
        private_data_size: u32,
        socket_id: i32,
        flags: u32,
    ) -> Result<Self> {
        #[allow(clippy::unwrap_used)]
        let name = CString::new(name).unwrap();
        let inner = MempoolInner::create(
            &name,
            n,
            elt_size,
            cache_size,
            private_data_size,
            socket_id,
            flags,
        )?;
        Ok(Self { inner })
    }

    /// Search a mempool from its name.
    #[inline]
    #[allow(clippy::unwrap_in_result)]
    pub fn lookup(name: &str) -> Result<Self> {
        #[allow(clippy::unwrap_used)]
        let name = CString::new(name).unwrap();
        let inner = MempoolInner::lookup(&name)?;
        Ok(Self { inner })
    }

    /// Get one object from the mempool.
    #[inline]
    pub fn get<T: MempoolObj>(&self) -> Result<T> {
        self.inner.get::<T>()
    }

    /// Put one object back in the mempool.
    #[inline]
    pub fn put(&self, obj: impl MempoolObj) {
        self.inner.put(obj);
    }

    /// Get several objects from the mempool.
    #[inline]
    pub fn get_bulk<T: MempoolObj>(&self, n: u32) -> Result<Vec<T>> {
        self.inner.get_bulk(n)
    }

    /// Put several objects back in the mempool.
    #[inline]
    pub fn put_bulk(&self, objs: &[impl MempoolObj], n: u32) {
        self.inner.put_bulk(objs, n);
    }

    /// Return the number of entries in the mempool. When cache is enabled, this function has to browse
    /// the length of all lcores, so it should not be used in a data path, but only for debug purposes.
    /// User-owned mempool caches are not accounted for.
    #[must_use]
    #[inline]
    pub fn available(&self) -> u32 {
        self.inner.avail_count()
    }

    /// Return the number of elements which have been allocated from the mempool.
    #[must_use]
    #[inline]
    pub fn in_use(&self) -> u32 {
        self.inner.in_use_count()
    }
    /// Test if the mempool is empty.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.empty()
    }

    /// Test if the mempool is full.
    #[must_use]
    #[inline]
    pub fn is_full(&self) -> bool {
        self.inner.full()
    }

    /// Get a pointer to `rte_mempool`.
    pub(crate) fn as_ptr(&self) -> *mut rte_mempool {
        self.inner.as_ptr()
    }

    /// Get a new instance of `Mempool`.
    pub(crate) fn new(inner: Arc<MempoolInner>) -> Self {
        Self { inner }
    }
}

/// Mempool, an allocator in DPDK.
#[derive(Clone)]
pub(crate) struct MempoolInner {
    /// A pointer to `rte_mempool`.
    mp: NonNull<rte_mempool>,
}

// SAFETY: mempool can be globally accessed
#[allow(unsafe_code)]
unsafe impl Send for MempoolInner {}

// SAFETY: mempool can be globally accessed
#[allow(unsafe_code)]
unsafe impl Sync for MempoolInner {}

impl Debug for MempoolInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mempool").finish()
    }
}

impl Drop for MempoolInner {
    fn drop(&mut self) {
        // SAFETY: ffi
        #[allow(unsafe_code)]
        unsafe {
            rte_mempool_free(self.mp.as_ptr());
        }
    }
}

/// Wrapper for `rte_mempool`.
#[allow(unsafe_code, clippy::unwrap_in_result)]
impl MempoolInner {
    /// Create a new `Mempool` with a pointer.
    pub(crate) fn new(ptr: *mut rte_mempool) -> Result<Arc<Self>> {
        let mp = NonNull::new(ptr).ok_or(Error::NoMem)?;
        let mp = Arc::new(Self { mp });
        #[allow(clippy::unwrap_used)]
        let _prev = MEMPOOLS
            .lock()
            .unwrap()
            .insert(ptr as usize, Arc::downgrade(&mp));
        Ok(mp)
    }

    /// Create a new `Mempool`.
    #[inline]
    fn create(
        name: &CString,
        n: u32,
        elt_size: u32,
        cache_size: u32,
        private_data_size: u32,
        socket_id: i32,
        flags: u32,
    ) -> Result<Arc<Self>> {
        // SAFETY: ffi
        let ptr = unsafe {
            rte_mempool_create(
                name.as_ptr(),
                n,
                elt_size,
                cache_size,
                private_data_size,
                None,
                ptr::null_mut(),
                None,
                ptr::null_mut(),
                socket_id,
                flags,
            )
        };
        Self::new(ptr)
    }

    /// Lookup a `Mempool` with its name.
    #[inline]
    #[allow(clippy::unwrap_in_result)]
    fn lookup(name: &CString) -> Result<Arc<Self>> {
        // SAFETY: ffi
        let ptr = unsafe { rte_mempool_lookup(name.as_ptr()) };
        if ptr.is_null() {
            return Err(Error::from_errno());
        }
        #[allow(clippy::unwrap_used)]
        MEMPOOLS
            .lock()
            .unwrap()
            .get(&(ptr as usize))
            .map(|weak| weak.upgrade().unwrap())
            .ok_or(Error::Unknown)
    }

    /// Put an object to the mempool.
    #[inline]
    fn put(&self, obj: impl MempoolObj) {
        self.put_bulk(&[obj], 1);
    }

    /// Put several objects back to the mempool.
    #[inline]
    fn put_bulk(&self, objs: &[impl MempoolObj], n: u32) {
        let mut obj_table = objs.iter().map(MempoolObj::as_c_void).collect::<Vec<_>>();
        // SAFETY: ffi
        #[allow(unsafe_code)]
        unsafe {
            rte_mempool_put_bulk(self.mp.as_ptr(), obj_table.as_mut_ptr(), n);
        }
    }

    /// Get an object from the mempool.
    #[inline]
    fn get<T: MempoolObj>(&self) -> Result<T> {
        let mut obj = MaybeUninit::<T>::uninit();
        // SAFETY: ffi
        let errno =
            unsafe { rte_mempool_get(self.mp.as_ptr(), obj.as_mut_ptr().cast::<*mut c_void>()) };
        Error::from_ret(errno)?;
        // SAFETY: objs are initialized since no error reported
        unsafe { Ok(obj.assume_init()) }
    }

    /// Get a bulk of objects.
    #[inline]
    fn get_bulk<T: MempoolObj>(&self, n: u32) -> Result<Vec<T>> {
        let mut objs = (0..n)
            .map(|_| MaybeUninit::<T>::uninit())
            .collect::<Vec<_>>();
        let mut obj_table = objs
            .iter_mut()
            .map(|obj| obj.as_mut_ptr().cast::<c_void>())
            .collect::<Vec<_>>();
        // SAFETY: ffi
        let errno = unsafe { rte_mempool_get_bulk(self.mp.as_ptr(), obj_table.as_mut_ptr(), n) };
        Error::from_ret(errno)?;
        // SAFETY: objs are initialized in `rte_mempool_get_bulk`
        Ok(objs
            .into_iter()
            .map(|obj| unsafe { obj.assume_init() })
            .collect())
    }

    /// The number of available objects.
    #[inline]
    fn avail_count(&self) -> u32 {
        // SAFETY: ffi
        unsafe { rte_mempool_avail_count(self.mp.as_ptr()) }
    }

    /// The number of objects that are in-use.
    #[inline]
    fn in_use_count(&self) -> u32 {
        // SAFETY: ffi
        unsafe { rte_mempool_in_use_count(self.mp.as_ptr()) }
    }

    /// The `Mempool` is empty or not.
    #[inline]
    fn empty(&self) -> bool {
        self.avail_count() == 0
    }

    /// The `Mempool` is full or not.
    #[inline]
    fn full(&self) -> bool {
        // SAFETY: the *rte_mempool pointer is valid
        unsafe { self.avail_count() == self.mp.as_ref().size }
    }

    /// Get inner pointer to `rte_mempool`.
    fn as_ptr(&self) -> *mut rte_mempool {
        self.mp.as_ptr()
    }
}

/// Mempool elements
#[allow(clippy::module_name_repetitions)]
pub trait MempoolObj: Sized {
    /// Transform objects into C pointers.
    fn as_c_void(&self) -> *mut c_void;
    /// Build an object from C pointer.
    fn from_c_void(ptr: *mut c_void) -> Self;
}

#[cfg(test)]
mod test {
    use crate::eal::{self, IovaMode};
    use crate::lcore;
    use crate::mempool::{self, Mempool};

    #[test]
    fn test() {
        let _ = eal::Config::new().iova_mode(IovaMode::VA).enter();
        let mp = Mempool::create(
            "mempool",
            64,
            16,
            0,
            0,
            lcore::socket_id(),
            mempool::MEMPOOL_SINGLE_CONSUMER | mempool::MEMPOOL_SINGLE_PRODUCER,
        )
        .unwrap();
        assert!(mp.is_full());
        assert_eq!(mp.in_use(), 0);
        assert_eq!(mp.available(), 64);

        let mp1 = Mempool::lookup("mempool").unwrap();
        assert!(mp1.is_full());
        assert_eq!(mp1.in_use(), 0);
        assert_eq!(mp1.available(), 64);
    }
}
