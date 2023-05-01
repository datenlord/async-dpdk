//! Memory pool is an allocator of fixed-sized objects. It is based on `rte_ring`, a lockless FIFO queue
//! in DPDK. It provides some other optional services such as a per-core object cache and an alignment
//! helper to ensure that objects are padded to spread them equally on all DRAM or DDR3 channels.
//!
//! In DPDK apps, mempools are widely used in the memory management for packet buffers.
//!
//! # Examples
//!
//! A simple example using mempool to store net packets:
//!
//! ```
//! # use async_dpdk::mbuf::Mbuf;
//! # use async_dpdk::mempool::{Mempool, PktMempool};
//!
//! # let _ = async_dpdk::eal::Config::new().enter();
//! let mp = PktMempool::create("pktmbuf", 512).unwrap();
//! let mut mbuf = Mbuf::new(&mp).unwrap();
//! // Append 10 bytes to `Mbuf`.
//! let data = mbuf.append(10).unwrap();
//! data.copy_from_slice("payloadxxx".as_bytes());
//! ```
//!
//! An example of using mempool as an allocator of self-defined objects:
//!
//! ```
//! # use async_dpdk::mempool::{GenericMempool, Mempool, MempoolObj};
//! # use std::os::raw::c_void;
//!
//! #[repr(C)]
//! #[derive(Default)]
//! struct SomeType {
//!     x: u64,
//!     y: u64,
//! }
//! struct SomePtr {
//!     ptr: *mut SomeType,
//! }
//! impl Default for SomePtr {
//!     fn default() -> Self {
//!         Self {
//!             ptr: std::ptr::null_mut(),
//!         }
//!     }
//! }
//! impl MempoolObj for SomePtr {
//!     fn into_raw(self) -> *mut c_void {
//!         self.ptr.cast()
//!     }
//!     fn from_raw(ptr: *mut c_void) -> Result<Self, async_dpdk::Error> {
//!         Ok(Self { ptr: ptr.cast() })
//!     }
//!     fn obj_size() -> usize {
//!         std::mem::size_of::<SomeType>()
//!     }
//! }
//!
//! # let _ = async_dpdk::eal::Config::new().enter();
//! let mp: GenericMempool<SomePtr> = GenericMempool::create("mempool", 64).unwrap();
//! let obj = mp.get().unwrap();
//! mp.put(obj);
//! ```

use crate::{lcore, mbuf::Mbuf, Error, Result};
use dpdk_sys::{
    rte_mempool, rte_mempool_avail_count, rte_mempool_create, rte_mempool_free, rte_mempool_get,
    rte_mempool_get_bulk, rte_mempool_in_use_count, rte_mempool_lookup, rte_mempool_put,
    rte_mempool_put_bulk, rte_pktmbuf_alloc, rte_pktmbuf_free, rte_pktmbuf_pool_create,
    RTE_MBUF_DEFAULT_BUF_SIZE,
};
use lazy_static::lazy_static;
use log::trace;
use std::{
    collections::HashMap,
    ffi::CString,
    fmt::Debug,
    marker::PhantomData,
    os::raw::c_void,
    ptr::{self, NonNull},
    sync::Mutex,
    sync::{Arc, Weak},
};

lazy_static! {
    static ref MEMPOOLS: Mutex<HashMap<usize, Weak<MpRef>>> = Mutex::default();
}

/// Objects allocated from a mempool.
///
/// In DPDK APIs, allocated objects are represented by pointers. So implementations should take care of
/// the convertion between a `MempoolObj` instance and its pointer. Also, it is **strongly recommended**
/// that `MempoolObj` should implement `Drop` trait, since an explicit call to DPDK API is needed to
/// deallocate the memory.
#[allow(clippy::module_name_repetitions)]
pub trait MempoolObj: Sized {
    /// Takes the ownership of `self` and convert to pointer.
    fn into_raw(self) -> *mut c_void;

    /// Convert to object.
    ///
    /// # Errors
    ///
    /// The pointer provided is invalid.
    fn from_raw(ptr: *mut c_void) -> Result<Self>;

    /// Size of the object.
    fn obj_size() -> usize;
}

/// Mempool is an allocator for fixed-sized objects and it is widely used in DPDK. For more
/// information, please refer to [`dpdk mempool docs`].
///
/// This trait allow users to define their own allocators. Also, it is **strongly recommanded**
/// that `Drop` trait should be implemented for `Mempool`s, since an explicit call is needed to
/// deallocate mempool resources.
///
/// [`dpdk mempool docs`]: https://doc.dpdk.org/guides/prog_guide/mempool_lib.html
pub trait Mempool<T: MempoolObj>: Sized {
    /// Create a new instance of `Mempool`.
    ///
    /// A name identifying the instance and capacity are needed. Implementations should call DPDK
    /// APIs such as `rte_mempool_create` and `rte_mempool_create_empty`.
    ///
    /// # Errors
    ///
    /// Possible errors:
    ///
    /// - No approporiate memory area left.
    /// - Called from a secondary process.
    /// - A memzone with the same name already exists.
    /// - The maximum number of memzones has already been allocated.
    fn create(name: &str, size: u32) -> Result<Self>;

    /// Get a mempool instance using name.
    ///
    /// # Errors
    ///
    /// This function could returns an error if the name does not match to any mempool.
    fn lookup(name: &str) -> Result<Self>;

    /// Allocate an object from mempool.
    ///
    /// # Errors
    ///
    /// This function could returns an error if the mempool is out of memory.
    fn get(&self) -> Result<T>;

    /// Deallocate an object.
    fn put(&self, object: T);

    /// Number of available objects.
    fn available(&self) -> u32;

    /// Number of objects in use.
    fn in_use(&self) -> u32;

    /// Whether the mempool is empty.
    fn is_empty(&self) -> bool;

    /// Whether the mempool is full.
    fn is_full(&self) -> bool;
}

/// Generic `MempoolObj` allocator.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug)]
pub struct GenericMempool<T>
where
    T: Default + MempoolObj,
{
    /// An `Arc` pointer to `MempoolInner`.
    inner: Arc<MpRef>,
    /// Placeholder for generic type.
    _marker: PhantomData<T>,
}

impl<T> Mempool<T> for GenericMempool<T>
where
    T: Default + MempoolObj,
{
    #[inline]
    fn create(name: &str, size: u32) -> Result<Self> {
        Self::new(name, size, 0, 0)
    }

    #[inline]
    fn lookup(name: &str) -> Result<Self> {
        let name = CString::new(name).map_err(Error::from)?;
        let inner = MpRef::lookup(&name)?;
        Ok(Self {
            inner,
            _marker: PhantomData,
        })
    }

    #[inline]
    fn get(&self) -> Result<T> {
        let mut ptr = ptr::null_mut::<T>();
        // SAFETY: invalid allocation result in a negative errno, which is checked later.
        // DPDK allocated objects are aligned to the cacheline size.
        #[allow(trivial_casts, unsafe_code)]
        let errno = unsafe {
            rte_mempool_get(
                self.inner.as_ptr(),
                ptr::addr_of_mut!(ptr).cast::<*mut c_void>(),
            )
        };
        Error::from_ret(errno)?;
        // SAFETY: valid memory, initialized here
        #[allow(unsafe_code)]
        unsafe {
            *ptr = T::default();
            T::from_raw(ptr.cast())
        }
    }

    #[inline]
    fn put(&self, object: T) {
        // SAFETY: *rte_mempool pointer checked
        #[allow(unsafe_code)]
        unsafe {
            rte_mempool_put(self.inner.as_ptr(), object.into_raw());
        }
    }

    #[must_use]
    #[inline]
    fn available(&self) -> u32 {
        self.inner.avail_count()
    }

    #[must_use]
    #[inline]
    fn in_use(&self) -> u32 {
        self.inner.in_use_count()
    }
    #[must_use]
    #[inline]
    fn is_empty(&self) -> bool {
        self.inner.empty()
    }

    #[must_use]
    #[inline]
    fn is_full(&self) -> bool {
        self.inner.full()
    }
}

impl<T> GenericMempool<T>
where
    T: Default + MempoolObj,
{
    /// Get a new instance.
    ///
    /// # Errors
    ///
    /// Possible errors: no approporiate memory area left, called from a secondary process, cache
    /// size provided is too large, a memzone with the same name already exists, the maximum number
    /// of memzones has already been allocated.
    #[inline]
    pub fn new(name: &str, size: u32, cache_size: u32, priv_size: u32) -> Result<Self> {
        let name = CString::new(name).map_err(Error::from)?;
        let obj_size = T::obj_size().try_into().map_err(Error::from)?;
        let socket_id = lcore::socket_id();

        // SAFETY: pointer checked in `MpRef::new`
        #[allow(unsafe_code)]
        let ptr = unsafe {
            rte_mempool_create(
                name.as_ptr(),
                size,
                obj_size,
                cache_size,
                priv_size,
                None,
                ptr::null_mut(),
                None,
                ptr::null_mut(),
                socket_id,
                0,
            )
        };

        trace!("A mempool with {size} elements of {obj_size} created");
        let inner = MpRef::new(ptr)?;
        Ok(Self {
            inner,
            _marker: PhantomData,
        })
    }

    /// Get several objects from the mempool.
    ///
    /// # Errors
    ///
    /// This function could returns an error if the mempool is out of memory.
    #[inline]
    pub fn get_bulk(&self, n: u32) -> Result<Vec<Box<T>>> {
        let mut ptrs = (0..n).map(|_| ptr::null_mut::<T>()).collect::<Vec<_>>();
        // SAFETY: invalid allocation result in a negative errno
        #[allow(unsafe_code)]
        let errno =
            unsafe { rte_mempool_get_bulk(self.inner.as_ptr(), ptrs.as_mut_ptr().cast(), n) };
        Error::from_ret(errno)?;
        let vec = ptrs
            .into_iter()
            .map(|ptr| {
                // SAFETY: pointers' validity checked
                #[allow(unsafe_code)]
                unsafe {
                    *ptr = T::default();
                    Box::from_raw(ptr)
                }
            })
            .collect();
        Ok(vec)
    }

    /// Put several objects back in the mempool.
    #[inline]
    pub fn put_bulk(&self, objs: Vec<T>, n: u32) {
        let mut objs: Vec<_> = objs.into_iter().map(MempoolObj::into_raw).collect();
        // SAFETY: *rte_mempool and *rte_mbuf checked
        #[allow(unsafe_code)]
        unsafe {
            rte_mempool_put_bulk(self.inner.as_ptr(), objs.as_mut_ptr().cast(), n);
        }
    }
}

/// Packet mempool.
#[allow(clippy::module_name_repetitions)]
#[derive(Debug)]
pub struct PktMempool {
    /// Inner pointer.
    inner: Arc<MpRef>,
}

impl Mempool<Mbuf> for PktMempool {
    #[inline]
    fn create(name: &str, size: u32) -> Result<Self> {
        let socket_id = lcore::socket_id();
        let name = CString::new(name).map_err(Error::from)?;
        // SAFETY: pointer checked in `MpRef::new`
        #[allow(unsafe_code, clippy::cast_possible_truncation)]
        let ptr = unsafe {
            rte_pktmbuf_pool_create(
                name.as_ptr(),
                size,
                0,
                0,
                RTE_MBUF_DEFAULT_BUF_SIZE as u16,
                socket_id,
            )
        };
        let inner = MpRef::new(ptr)?;
        Ok(Self::new(inner))
    }

    #[inline]
    fn lookup(name: &str) -> Result<Self> {
        let name = CString::new(name).map_err(Error::from)?;
        let inner = MpRef::lookup(&name)?;
        Ok(Self { inner })
    }

    #[inline]
    fn get(&self) -> Result<Mbuf> {
        // SAFETY: pointer checked in `Mbuf::new_with_ptr`
        #[allow(unsafe_code)]
        let ptr = unsafe { rte_pktmbuf_alloc(self.as_ptr()) };
        Mbuf::new_with_ptr(ptr)
    }

    #[inline]
    fn put(&self, object: Mbuf) {
        // SAFETY: ffi
        #[allow(unsafe_code)]
        unsafe {
            rte_pktmbuf_free(object.as_ptr());
        }
    }

    #[inline]
    fn available(&self) -> u32 {
        self.inner.avail_count()
    }

    #[inline]
    fn in_use(&self) -> u32 {
        self.inner.in_use_count()
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.inner.empty()
    }

    #[inline]
    fn is_full(&self) -> bool {
        self.inner.full()
    }
}

impl PktMempool {
    /// Get a pointer to `rte_mempool`.
    #[inline]
    pub(crate) fn as_ptr(&self) -> *mut rte_mempool {
        self.inner.as_ptr()
    }

    /// Get a new instance of `Mempool`.
    #[inline]
    pub(crate) fn new(inner: Arc<MpRef>) -> Self {
        Self { inner }
    }
}

/// `MempoolRef` is a wrapper of `*rte_mempool`. It is mapped to one instance of `rte_mempool`.
///
/// Since `Mempool`s can be found using names, a `MempoolRef` can be held by several `Mempool`s.
/// A global hash table storing `Weak` pointers is used to track the ref count of `Mempool`s.
#[derive(Clone)]
pub struct MpRef {
    /// A pointer to `rte_mempool`.
    mp: NonNull<rte_mempool>,
}

// SAFETY: mempool can be globally accessed
#[allow(unsafe_code)]
unsafe impl Send for MpRef {}

// SAFETY: mempool can be globally accessed
#[allow(unsafe_code)]
unsafe impl Sync for MpRef {}

impl Debug for MpRef {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mempool").finish()
    }
}

impl MpRef {
    /// Create a new `MempoolInner` instance with a pointer.
    fn new(ptr: *mut rte_mempool) -> Result<Arc<Self>> {
        let mp = NonNull::new(ptr).ok_or(Error::NoMem)?;
        let mp = Arc::new(Self { mp });
        let _prev = MEMPOOLS
            .lock()
            .map_err(Error::from)?
            .insert(ptr as usize, Arc::downgrade(&mp));
        Ok(mp)
    }

    /// Lookup a `Mempool` with its name.
    #[inline]
    fn lookup(name: &CString) -> Result<Arc<Self>> {
        // SAFETY: pointer checked later
        #[allow(unsafe_code)]
        let ptr = unsafe { rte_mempool_lookup(name.as_ptr()) };
        if ptr.is_null() {
            return Err(Error::from_errno());
        }
        if let Some(weak) = MEMPOOLS.lock().map_err(Error::from)?.get(&(ptr as usize)) {
            Ok(weak.upgrade().ok_or(Error::NotExist)?)
        } else {
            Err(Error::NotExist)
        }
    }

    /// The number of available objects.
    #[inline]
    fn avail_count(&self) -> u32 {
        // SAFETY: *rte_mempool checked
        #[allow(unsafe_code)]
        unsafe {
            rte_mempool_avail_count(self.mp.as_ptr())
        }
    }

    /// The number of objects that are in-use.
    #[inline]
    fn in_use_count(&self) -> u32 {
        // SAFETY: *rte_mempool checked
        #[allow(unsafe_code)]
        unsafe {
            rte_mempool_in_use_count(self.mp.as_ptr())
        }
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
        #[allow(unsafe_code)]
        unsafe {
            self.avail_count() == self.mp.as_ref().size
        }
    }

    /// Get inner pointer to `rte_mempool`.
    fn as_ptr(&self) -> *mut rte_mempool {
        self.mp.as_ptr()
    }
}

impl Drop for MpRef {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: *rte_mempool checked
        #[allow(unsafe_code)]
        unsafe {
            rte_mempool_free(self.mp.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::mem;
    use std::os::raw::c_void;
    use std::ptr;

    use crate::mempool::{GenericMempool, Mempool};
    use crate::test_utils;

    use super::MempoolObj;

    #[repr(C)]
    struct SomeType {
        x: u64,
        y: u64,
        d: [u32; 10],
    }
    struct SomePtr {
        ptr: *mut SomeType,
    }
    impl Default for SomePtr {
        fn default() -> Self {
            Self {
                ptr: ptr::null_mut(),
            }
        }
    }
    impl MempoolObj for SomePtr {
        fn into_raw(self) -> *mut c_void {
            self.ptr.cast()
        }
        fn from_raw(ptr: *mut c_void) -> crate::Result<Self> {
            Ok(Self { ptr: ptr.cast() })
        }
        fn obj_size() -> usize {
            mem::size_of::<SomeType>()
        }
    }

    #[test]
    fn test() {
        test_utils::dpdk_setup();
        let mp: GenericMempool<SomePtr> = GenericMempool::create("mempool", 64).unwrap();
        assert!(mp.is_full());
        assert!(!mp.is_empty());
        assert_eq!(mp.in_use(), 0);
        assert_eq!(mp.available(), 64);

        let mp1: GenericMempool<SomePtr> = GenericMempool::lookup("mempool").unwrap();
        assert!(mp1.is_full());
        assert_eq!(mp1.in_use(), 0);
        assert_eq!(mp1.available(), 64);

        let obj = mp.get().unwrap();
        assert_eq!(mp1.in_use(), 1);
        assert_eq!(mp1.available(), 63);

        mp.put(obj);
        assert!(mp1.is_full());
    }
}
