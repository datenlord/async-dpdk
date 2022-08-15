//! mempool wrapper

use crate::CString;
use crate::Error;
use dpdk_sys::*;
use lazy_static::lazy_static;
use std::collections::HashMap;
use std::fmt::Debug;
use std::mem::MaybeUninit;
use std::os::raw::c_void;
use std::sync::Mutex;
use std::{
    ptr::{self, NonNull},
    sync::{Arc, Weak},
};

lazy_static! {
    static ref MEMPOOLS: Mutex<HashMap<usize, Weak<MempoolInner>>> = Default::default();
}

/// Mempool is an allocator for fixed sized object.In DPDK, it's identified by name and uses a
/// mempool handler to store free objects. It provides some other optional services such as
/// per-core object cache and an alignment helper.
#[derive(Debug)]
pub struct Mempool {
    inner: Arc<MempoolInner>,
}

impl Mempool {
    /// Create a new Mempool named `name` in memory.
    pub fn create(
        name: &str,
        n: u32,
        elt_size: u32,
        cache_size: u32,
        private_data_size: u32,
        socket_id: i32,
        flags: u32,
    ) -> Result<Self, Error> {
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
        Ok(Self { inner })
    }

    /// Search a mempool from its name.
    pub fn lookup(name: &str) -> Result<Self, Error> {
        let name = CString::new(name).unwrap();
        let inner = MempoolInner::lookup(name)?;
        Ok(Self { inner })
    }

    /// Get mempool from object.
    pub fn from_object(obj: &impl MempoolObj) -> Result<Self, Error> {
        let inner = MempoolInner::from_object(obj)?;
        Ok(Self { inner })
    }

    /// Get one object from the mempool.
    pub fn get<T: MempoolObj>(&self) -> Result<T, Error> {
        self.inner.get::<T>()
    }

    /// Put one object back in the mempool.
    pub fn put(&self, obj: impl MempoolObj) {
        self.inner.put(obj)
    }

    /// Get several objects from the mempool.
    pub fn get_bulk<T: MempoolObj>(&self, n: u32) -> Result<Vec<T>, Error> {
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

    pub(crate) fn new(inner: Arc<MempoolInner>) -> Self {
        Self { inner }
    }
}

/// Mempool
#[derive(Clone)]
#[allow(missing_copy_implementations)]
pub struct MempoolInner {
    mp: NonNull<rte_mempool>,
}

#[allow(unsafe_code)]
unsafe impl Send for MempoolInner {}

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
            rte_mempool_free(self.mp.as_ptr());
        }
    }
}

#[allow(unsafe_code)]
impl MempoolInner {
    pub(crate) fn new(ptr: *mut rte_mempool) -> Result<Arc<Self>, Error> {
        let mp = NonNull::new(ptr).map_or(Err(Error::NoMem), |mp| Ok(mp))?;
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
    ) -> Result<Arc<Self>, Error> {
        // TODO: flags
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
    fn lookup(name: CString) -> Result<Arc<Self>, Error> {
        let ptr = unsafe { rte_mempool_lookup(name.as_ptr()) };
        MEMPOOLS
            .lock()
            .unwrap()
            .get(&(ptr as usize))
            .map(|weak| weak.upgrade().unwrap())
            .map_or(Err(Error::NotExist), |mp| Ok(mp))
    }

    #[inline(always)]
    fn from_object(obj: &impl MempoolObj) -> Result<Arc<Self>, Error> {
        let ptr = unsafe { rte_mempool_from_obj(obj.as_c_void()) };
        MEMPOOLS
            .lock()
            .unwrap()
            .get(&(ptr as usize))
            .map(|weak| weak.upgrade().unwrap())
            .map_or(Err(Error::NotExist), |mp| Ok(mp))
    }

    #[allow(dead_code)]
    #[inline(always)]
    fn obj_iter(&self, f: rte_mempool_obj_cb_t, args: *mut c_void) {
        let _ = unsafe { rte_mempool_obj_iter(self.mp.as_ptr(), f, args) };
    }

    #[allow(dead_code)]
    #[inline(always)]
    fn mem_iter(&self, f: rte_mempool_mem_cb_t, args: *mut c_void) {
        let _ = unsafe { rte_mempool_mem_iter(self.mp.as_ptr(), f, args) };
    }

    /// Put an object to the mempool.
    #[inline(always)]
    pub fn put(&self, obj: impl MempoolObj) {
        self.put_bulk(vec![obj], 1);
    }

    /// Put several objects back to the mempool.
    #[inline(always)]
    pub fn put_bulk(&self, objs: Vec<impl MempoolObj>, n: u32) {
        let mut obj_table = objs.iter().map(|obj| obj.as_c_void()).collect::<Vec<_>>();
        #[allow(unsafe_code)]
        unsafe {
            rte_mempool_put_bulk(self.mp.as_ptr(), obj_table.as_mut_ptr(), n);
        }
    }

    /// Get an object from the mempool.
    #[inline(always)]
    pub fn get<T: MempoolObj>(&self) -> Result<T, Error> {
        let mut obj = MaybeUninit::<T>::uninit();
        unsafe {
            let errno = rte_mempool_get(self.mp.as_ptr(), obj.as_mut_ptr() as *mut *mut c_void);
            Error::from_errno(errno)?;
            Ok(obj.assume_init())
        }
    }

    #[inline(always)]
    fn get_bulk<T: MempoolObj>(&self, n: u32) -> Result<Vec<T>, Error> {
        let mut objs = (0..n)
            .map(|_| MaybeUninit::<T>::uninit())
            .collect::<Vec<_>>();
        let mut obj_table = objs
            .iter_mut()
            .map(|obj| obj.as_mut_ptr() as *mut c_void)
            .collect::<Vec<_>>();
        let errno = unsafe { rte_mempool_get_bulk(self.mp.as_ptr(), obj_table.as_mut_ptr(), n) };
        Error::from_errno(errno)?;
        Ok(objs
            .into_iter()
            .map(|obj| unsafe { obj.assume_init() })
            .collect())
    }

    #[inline(always)]
    fn name(&self) -> CString {
        unsafe { CString::from_raw((*self.mp.as_ptr()).name.as_mut_ptr()) }
    }

    #[inline(always)]
    fn avail_count(&self) -> u32 {
        unsafe { rte_mempool_avail_count(self.mp.as_ptr()) }
    }

    #[inline(always)]
    fn in_use_count(&self) -> u32 {
        unsafe { rte_mempool_in_use_count(self.mp.as_ptr()) }
    }

    #[inline(always)]
    fn empty(&self) -> bool {
        self.avail_count() == 0
    }

    #[inline(always)]
    fn full(&self) -> bool {
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
}
