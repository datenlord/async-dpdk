//! EAL (Environment Abstract Layer) in DPDK is responsible for gaining access to low-level
//! resources such as hardware and memory space. It provides a generic interface that
//! hides the environment specifics from the applications and libraries.
//!
//! EAL provides services such as DPDK loading and launching, system memory reservation,
//! interrupt handling and core affinity assignment.
//!
//! This module deals with the configuring and launching of EAL. Users should first enter
//! EAL environment before dealing with any DPDK provided features.
//!
//! # Example
//!
//! A simple example of setting up the environment:
//!
//! ```
//! use async_dpdk::eal::{self, IovaMode};
//!
//! eal::Config::new()
//!     .enter()
//!     .coremask(0x3f)
//!     .device_probe(&["192.168.0.1", "192.168.0.2"])
//!     .iova_mode(IovaMode::VA)
//!     .unwrap();
//! ```

use crate::{Error, Result};
use dpdk_sys::{
    rte_eal_cleanup, rte_eal_get_runtime_dir, rte_eal_has_hugepages, rte_eal_has_pci, rte_eal_init,
};
use lazy_static::lazy_static;
use log::error;
use std::ffi::CString;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::{os::raw::c_char, path::PathBuf};

lazy_static! {
    static ref CONTEXT: RwLock<Option<Arc<Eal>>> = RwLock::new(None);
}

/// The existence of an `Eal` instance shows the readiness of a DPDK environment.
#[non_exhaustive]
#[derive(Debug)]
pub struct Eal {}

#[allow(unsafe_code)]
unsafe impl Sync for Eal {}

/// Whether EAL is using hugepages.
#[allow(unsafe_code)]
#[inline]
#[must_use]
pub fn has_hugepages() -> bool {
    // SAFETY: ffi
    unsafe { rte_eal_has_hugepages() != 0 }
}

/// Whether EAL is using PCI bus.
#[allow(unsafe_code)]
#[inline]
#[must_use]
pub fn has_pci() -> bool {
    // SAFETY: ffi
    unsafe { rte_eal_has_pci() != 0 }
}

/// Get the runtime directory of DPDK
#[allow(unsafe_code)]
#[inline]
pub fn runtime_dir() -> Result<PathBuf> {
    // SAFETY: ffi
    let ptr = unsafe { rte_eal_get_runtime_dir() };
    // SAFETY: read C string
    let cs = unsafe { CString::from_raw(ptr as _) };
    Ok(PathBuf::from(cs.into_string().map_err(Error::from)?))
}

impl Drop for Eal {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: ffi
        #[allow(unsafe_code)]
        let errno = unsafe { rte_eal_cleanup() };
        Error::parse_err(errno);
    }
}

/// Used for the configuration of EAL.
#[derive(Debug, Default)]
pub struct Config {
    /// Args passed to `rte_eal_init`.
    args: Vec<CString>,
    /// IP addresses for each `EthDev`s.
    addrs: Vec<IpAddr>,
}

/// IOVA mode.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::exhaustive_enums)]
pub enum IovaMode {
    /// physical address
    PA,
    /// virtual address
    VA,
}

/// DPDK log level.
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
#[allow(clippy::exhaustive_enums)]
pub enum LogLevel {
    /// System is unusable
    Emergency = 1,
    /// Action must be taken immediately
    Alert = 2,
    /// Critical conditions
    Critical = 3,
    /// Error conditions
    Error = 4,
    /// Warning conditions
    Warn = 5,
    /// Normal but significant conditions
    Notice = 6,
    /// Informational
    Info = 7,
    /// Debug-level messages
    Debug = 8,
}

impl Config {
    /// Create a new eal builder.
    #[inline]
    #[must_use]
    #[allow(clippy::unwrap_used, clippy::missing_panics_doc)]
    pub fn new() -> Self {
        let env_args = std::env::args().collect::<Vec<_>>();
        Self {
            args: vec![
                #[allow(clippy::indexing_slicing)] // the first of env args is the program name
                CString::new(env_args[0].as_str()).unwrap(),
            ],
            addrs: vec![],
        }
    }

    /// Probe devices or not.
    #[inline]
    pub fn device_probe(mut self, addr_str: &[&str]) -> Result<Self> {
        for addr in addr_str.iter() {
            self.addrs
                .push(IpAddr::from_str(addr).map_err(Error::from)?);
        }
        Ok(self)
    }

    /// Set core mask to EAL.
    #[inline]
    #[must_use]
    #[allow(clippy::unwrap_used, clippy::missing_panics_doc)] // impossible to panic
    pub fn coremask(mut self, mask: u64) -> Self {
        self.args.push(CString::new("-c").unwrap());
        self.args.push(CString::new(mask.to_string()).unwrap());
        self
    }

    /// Set core list to EAL.
    #[inline]
    pub fn corelist(mut self, list: &str) -> Result<Self> {
        self.args.push(CString::new("-l").map_err(Error::from)?);
        self.args.push(CString::new(list).map_err(Error::from)?);
        Ok(self)
    }

    /// Set core map to EAL.
    #[inline]
    pub fn coremap(mut self, map: &str) -> Result<Self> {
        self.args
            .push(CString::new("-lcores").map_err(Error::from)?);
        self.args.push(CString::new(map).map_err(Error::from)?);
        Ok(self)
    }

    /// Set pci blacklist.
    #[inline]
    pub fn pci_blacklist(mut self, name: &str) -> Result<Self> {
        self.args.push(CString::new("-b").map_err(Error::from)?);
        self.args.push(CString::new(name).map_err(Error::from)?);
        Ok(self)
    }

    /// Set pci whitelist.
    #[inline]
    pub fn pci_whitelist(mut self, name: &str) -> Result<Self> {
        self.args.push(CString::new("-w").map_err(Error::from)?);
        self.args.push(CString::new(name).map_err(Error::from)?);
        Ok(self)
    }

    /// Disable PCI.
    #[inline]
    #[must_use]
    #[allow(clippy::unwrap_used, clippy::missing_panics_doc)] // impossible to panic
    pub fn no_pci(mut self, no_pci: bool) -> Self {
        if no_pci {
            self.args.push(CString::new("--no-pci").unwrap());
        }
        self
    }

    /// Reserved memory on start in megabytes.
    #[inline]
    #[must_use]
    #[allow(clippy::unwrap_used, clippy::missing_panics_doc)] // impossible to panic
    pub fn memory_mb(mut self, size: u32) -> Self {
        self.args.push(CString::new("-m").unwrap());
        self.args.push(CString::new(size.to_string()).unwrap());
        self
    }

    /// Set iova mode.
    #[inline]
    #[must_use]
    #[allow(clippy::unwrap_used, clippy::missing_panics_doc)] // impossible to panic
    pub fn iova_mode(mut self, mode: IovaMode) -> Self {
        self.args.push(CString::new("--iova-mode").unwrap());
        match mode {
            IovaMode::PA => self.args.push(CString::new("pa").unwrap()),
            IovaMode::VA => self.args.push(CString::new("va").unwrap()),
        }
        self
    }

    /// Set log level.
    #[inline]
    #[must_use]
    #[allow(clippy::unwrap_used, clippy::missing_panics_doc)] // impossible to panic
    pub fn log_level(mut self, log_level: LogLevel) -> Self {
        self.args.push(CString::new("--log-level").unwrap());
        self.args
            .push(CString::new((log_level as u32).to_string()).unwrap());
        self
    }

    /// Initialize the Environment Abstraction Layer (EAL). This function is to be executed on
    /// the MAIN lcore only, as soon as possible in the application's `main()` function.
    #[inline]
    pub fn enter(self) -> Result<()> {
        if CONTEXT.read().map_err(Error::from)?.is_some() {
            return Err(Error::Already);
        }
        let mut pargs = self
            .args
            .iter()
            .map(|s| s.as_ptr() as *mut c_char)
            .collect::<Vec<_>>();

        if pargs.len() > i32::MAX as usize {
            return Err(Error::TooBig);
        }
        // SAFETY: ffi
        #[allow(unsafe_code)]
        let ret = unsafe {
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            // arg length checked
            rte_eal_init(pargs.len() as _, pargs.as_mut_ptr())
        };
        if ret < 0 {
            error!("Error initializing DPDK environment");
            return Err(Error::from_errno());
        }
        let context = Arc::new(Eal {});
        *CONTEXT.write().map_err(Error::from)? = Some(context);
        Ok(())
    }
}
