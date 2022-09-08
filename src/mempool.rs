//! mempool wrapper

use crate::eal::Eal;
use crate::{Error, Result};
use dpdk_sys::*;
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
    static ref MEMPOOLS: Mutex<HashMap<usize, Weak<MempoolInner>>> = Default::default();
}

/// Mempool flag. By default, objects addresses are spread between channels in RAM: the pool
/// allocator will add padding between objects depending on the hardware configuration. If this flag
/// is set, the allocator will just align them to a cache line.
pub const MEMPOOL_NO_SPREAD: u32 = RTE_MEMPOOL_F_NO_SPREAD;

/// By default, the returned objects are cache-aligned. This flag removes this constraint,
/// and no padding will be present between objects. This flag implies RTE_MEMPOOL_F_NO_SPREAD.
pub const MEMPOOL_NO_CACHE_ALIGN: u32 = RTE_MEMPOOL_F_NO_CACHE_ALIGN;

/// If this flag is set, the default behavior when using rte_mempool_put() or rte_mempool_put_bulk()
/// is "single-producer". Otherwise, it is "multi-producers".
pub const MEMPOOL_SINGLE_PRODUCER: u32 = RTE_MEMPOOL_F_SP_PUT;

/// If this flag is set, the default behavior when using rte_mempool_get() or rte_mempool_get_bulk()
/// is "single-consumer". Otherwise, it is "multi-consumers".
pub const MEMPOOL_SINGLE_CONSUMER: u32 = RTE_MEMPOOL_F_SC_GET;

/// If set, allocated objects won't necessarily be contiguous in IO memory.
pub const MEMPOOL_NO_IOVA_CONTIG: u32 = RTE_MEMPOOL_F_NO_IOVA_CONTIG;

/// Mempool is an allocator for fixed sized object.In DPDK, it's identified by name and uses a
/// mempool handler to store free objects. It provides some other optional services such as
/// per-core object cache and an alignment helper.
#[derive(Debug)]
pub struct Mempool {
    inner: Arc<MempoolInner>,
    _ctx: Arc<Eal>,
}

impl Mempool {
    /// Create a new Mempool named `name` in memory.
    pub fn create(
        ctx: &Arc<Eal>,
        name: &str,
        n: u32,
        elt_size: u32,
        cache_size: u32,
        private_data_size: u32,
        socket_id: i32,
        flags: u32,
    ) -> Result<Self> {
        let name = CString::new(name).unwrap();
        let inner = MempoolInner::create(
            name,
            n,
            elt_size,
            cache_size,
            private_data_size,
            socket_id,
            flags,
        )?;
        Ok(Self {
            inner,
            _ctx: ctx.clone(),
        })
    }

    /// Search a mempool from its name.
    pub fn lookup(ctx: &Arc<Eal>, name: &str) -> Result<Self> {
        let name = CString::new(name).unwrap();
        let inner = MempoolInner::lookup(name)?;
        Ok(Self {
            inner,
            _ctx: ctx.clone(),
        })
    }

    /// Get one object from the mempool.
    pub fn get<T: MempoolObj>(&self) -> Result<T> {
        self.inner.get::<T>()
    }

    /// Put one object back in the mempool.
    pub fn put(&self, obj: impl MempoolObj) {
        self.inner.put(obj)
    }

    /// Get several objects from the mempool.
    pub fn get_bulk<T: MempoolObj>(&self, n: u32) -> Result<Vec<T>> {
        self.inner.get_bulk(n)
    }

    /// Put several objects back in the mempool.
    pub fn put_bulk(&self, objs: Vec<impl MempoolObj>, n: u32) {
        self.inner.put_bulk(objs, n);
    }

    /// Get name pf the mempool.
    pub fn name(&self) -> String {
        self.inner.name().into_string().unwrap()
    }

    /// Return the number of entries in the mempool. When cache is enabled, this function has to browse
    /// the length of all lcores, so it should not be used in a data path, but only for debug purposes.
    /// User-owned mempool caches are not accounted for.
    pub fn available(&self) -> u32 {
        self.inner.avail_count()
    }

    /// Return the number of elements which have been allocated from the mempool.
    pub fn in_use(&self) -> u32 {
        self.inner.in_use_count()
    }
    /// Test if the mempool is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.empty()
    }

    /// Test if the mempool is full.
    pub fn is_full(&self) -> bool {
        self.inner.full()
    }

    pub(crate) fn as_ptr(&self) -> *mut rte_mempool {
        self.inner.as_ptr()
    }

    pub(crate) fn new(ctx: &Arc<Eal>, inner: Arc<MempoolInner>) -> Self {
        Self {
            inner,
            _ctx: ctx.clone(),
        }
    }
}

/// Mempool
#[derive(Clone)]
pub(crate) struct MempoolInner {
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
        #[allow(unsafe_code)]
        unsafe {
            // SAFETY: ffi
            rte_mempool_free(self.mp.as_ptr());
        }
    }
}

#[allow(unsafe_code)]
impl MempoolInner {
    pub(crate) fn new(ptr: *mut rte_mempool) -> Result<Arc<Self>> {
        let mp = NonNull::new(ptr).map_or_else(|| Err(Error::from_errno()), |mp| Ok(mp))?;
        let mp = Arc::new(Self { mp });
        assert!(MEMPOOLS
            .lock()
            .unwrap()
            .insert(ptr as usize, Arc::downgrade(&mp))
            .is_none());
        Ok(mp)
    }

    #[inline(always)]
    fn create(
        name: CString,
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

    #[inline(always)]
    fn lookup(name: CString) -> Result<Arc<Self>> {
        // SAFETY: ffi
        let ptr = unsafe { rte_mempool_lookup(name.as_ptr()) };
        if ptr.is_null() {
            return Err(Error::from_errno());
        }
        MEMPOOLS
            .lock()
            .unwrap()
            .get(&(ptr as usize))
            .map(|weak| weak.upgrade().unwrap())
            .map_or(Err(Error::Unknown), |mp| Ok(mp))
    }

    /// Put an object to the mempool.
    #[inline(always)]
    fn put(&self, obj: impl MempoolObj) {
        self.put_bulk(vec![obj], 1);
    }

    /// Put several objects back to the mempool.
    #[inline(always)]
    fn put_bulk(&self, objs: Vec<impl MempoolObj>, n: u32) {
        let mut obj_table = objs.iter().map(|obj| obj.as_c_void()).collect::<Vec<_>>();
        // SAFETY: ffi
        #[allow(unsafe_code)]
        unsafe {
            rte_mempool_put_bulk(self.mp.as_ptr(), obj_table.as_mut_ptr(), n);
        }
    }

    /// Get an object from the mempool.
    #[inline(always)]
    fn get<T: MempoolObj>(&self) -> Result<T> {
        let mut obj = MaybeUninit::<T>::uninit();
        // SAFETY: ffi
        let errno =
            unsafe { rte_mempool_get(self.mp.as_ptr(), obj.as_mut_ptr() as *mut *mut c_void) };
        Error::from_ret(errno)?;
        // SAFETY: objs are initialized
        unsafe { Ok(obj.assume_init()) }
    }

    #[inline(always)]
    fn get_bulk<T: MempoolObj>(&self, n: u32) -> Result<Vec<T>> {
        let mut objs = (0..n)
            .map(|_| MaybeUninit::<T>::uninit())
            .collect::<Vec<_>>();
        let mut obj_table = objs
            .iter_mut()
            .map(|obj| obj.as_mut_ptr() as *mut c_void)
            .collect::<Vec<_>>();
        // SAFETY: ffi
        let errno = unsafe { rte_mempool_get_bulk(self.mp.as_ptr(), obj_table.as_mut_ptr(), n) };
        Error::from_ret(errno)?;
        // SAFETY: objs are initialized
        Ok(objs
            .into_iter()
            .map(|obj| unsafe { obj.assume_init() })
            .collect())
    }

    #[inline(always)]
    fn name(&self) -> CString {
        // SAFETY: read C string
        unsafe { CString::from_raw((*self.mp.as_ptr()).name.as_mut_ptr()) } // BUG
    }

    #[inline(always)]
    fn avail_count(&self) -> u32 {
        // SAFETY: ffi
        unsafe { rte_mempool_avail_count(self.mp.as_ptr()) }
    }

    #[inline(always)]
    fn in_use_count(&self) -> u32 {
        // SAFETY: ffi
        unsafe { rte_mempool_in_use_count(self.mp.as_ptr()) }
    }

    #[inline(always)]
    fn empty(&self) -> bool {
        self.avail_count() == 0
    }

    #[inline(always)]
    fn full(&self) -> bool {
        // SAFETY: the *rte_mempool pointer is valid
        unsafe { self.avail_count() == self.mp.as_ref().size }
    }

    fn as_ptr(&self) -> *mut rte_mempool {
        self.mp.as_ptr()
    }
}

/// Mempool elements
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
        let eal = eal::Builder::new().iova_mode(IovaMode::VA).build().unwrap();

        let mp = Mempool::create(
            &eal,
            "mempool",
            64,
            16,
            0,
            0,
            lcore::socket_id() as _,
            mempool::MEMPOOL_SINGLE_CONSUMER | mempool::MEMPOOL_SINGLE_PRODUCER,
        )
        .unwrap();
        assert!(mp.is_full());
        assert_eq!(mp.in_use(), 0);
        assert_eq!(mp.available(), 64);

        let mp1 = Mempool::lookup(&eal, "mempool").unwrap();
        assert!(mp1.is_full());
        assert_eq!(mp1.in_use(), 0);
        assert_eq!(mp1.available(), 64);
    }
}
