//! EAL (Environment Abstract Layer)

use crate::net_dev;
use crate::{Error, Result};
use dpdk_sys::{
    rte_eal_cleanup, rte_eal_get_runtime_dir, rte_eal_has_hugepages, rte_eal_has_pci, rte_eal_init,
    rte_mp_disable,
};
use lazy_static::lazy_static;
use std::ffi::CString;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::{os::raw::c_char, path::PathBuf};

lazy_static! {
    static ref CONTEXT: RwLock<Option<Arc<Eal>>> = RwLock::new(None);
}

/// EAL
#[derive(Debug)]
#[allow(clippy::exhaustive_structs)]
pub struct Eal {}

#[allow(unsafe_code)]
unsafe impl Sync for Eal {}

/// Disable multiprocess.
///
/// This function can be called to indicate that multiprocess won't be used for the rest of
/// the application life.
#[allow(unsafe_code)]
#[inline]
#[must_use]
pub fn disable_mp() -> bool {
    // SAFETY: ffi
    unsafe { rte_mp_disable() }
}

/// Whether EAL is using hugepages.
#[allow(unsafe_code)]
#[inline]
#[must_use]
pub fn has_hugepages() -> bool {
    // SAFETY: ffi
    unsafe { rte_eal_has_hugepages() != 0 }
}

/// Whether EAL is using PCI bus. Disabled by –no-pci option.
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
#[must_use]
pub fn runtime_dir() -> PathBuf {
    // SAFETY: ffi
    let ptr = unsafe { rte_eal_get_runtime_dir() };
    // SAFETY: read C string
    let cs = unsafe { CString::from_raw(ptr as _) };
    #[allow(clippy::unwrap_used)]
    PathBuf::from(cs.into_string().unwrap())
}

impl Drop for Eal {
    #[inline]
    fn drop(&mut self) {
        // Close all devices
        net_dev::device_close();
        // SAFETY: ffi
        #[allow(unsafe_code)]
        let errno = unsafe { rte_eal_cleanup() };
        Error::parse_err(errno);
    }
}

/// EAL Builder
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

#[allow(clippy::unwrap_used)] // Create `CString`s
impl Config {
    /// Create a new eal builder.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        let env_args = std::env::args().collect::<Vec<_>>();
        Self {
            args: vec![CString::new(env_args[0].as_str()).unwrap()],
            addrs: vec![],
        }
    }

    /// Probe devices or not.
    #[inline]
    #[must_use]
    pub fn device_probe(mut self, addr_str: &[&str]) -> Self {
        for addr in addr_str.iter() {
            self.addrs.push(IpAddr::from_str(addr).unwrap());
        }
        self
    }

    /// Set core mask to EAL.
    #[inline]
    #[must_use]
    pub fn coremask(mut self, mask: u64) -> Self {
        self.args.push(CString::new("-c").unwrap());
        self.args.push(CString::new(mask.to_string()).unwrap());
        self
    }

    /// Set core list to EAL.
    #[inline]
    #[must_use]
    pub fn corelist(mut self, list: &str) -> Self {
        self.args.push(CString::new("-l").unwrap());
        self.args.push(CString::new(list).unwrap());
        self
    }

    /// Set core map to EAL.
    #[inline]
    #[must_use]
    pub fn coremap(mut self, map: &str) -> Self {
        self.args.push(CString::new("-lcores").unwrap());
        self.args.push(CString::new(map).unwrap());
        self
    }

    /// Set pci blacklist.
    #[inline]
    #[must_use]
    pub fn pci_blacklist(mut self, name: &str) -> Self {
        self.args.push(CString::new("-b").unwrap());
        self.args.push(CString::new(name).unwrap());
        self
    }

    /// Set pci whitelist.
    #[inline]
    #[must_use]
    pub fn pci_whitelist(mut self, name: &str) -> Self {
        self.args.push(CString::new("-w").unwrap());
        self.args.push(CString::new(name).unwrap());
        self
    }

    /// Disable PCI.
    #[inline]
    #[must_use]
    pub fn no_pci(mut self, no_pci: bool) -> Self {
        if no_pci {
            self.args.push(CString::new("--no-pci").unwrap());
        }
        self
    }

    /// Reserved memory on start in megabytes.
    #[inline]
    #[must_use]
    pub fn memory_mb(mut self, size: u32) -> Self {
        self.args.push(CString::new("-m").unwrap());
        self.args.push(CString::new(size.to_string()).unwrap());
        self
    }

    /// Set iova mode.
    #[inline]
    #[must_use]
    pub fn iova_mode(mut self, mode: IovaMode) -> Self {
        self.args.push(CString::new("--iova-mode").unwrap());
        match mode {
            IovaMode::PA => self.args.push(CString::new("pa").unwrap()),
            IovaMode::VA => self.args.push(CString::new("va").unwrap()),
        }
        self
    }

    /// Set log level
    #[inline]
    #[must_use]
    pub fn log_level(mut self, log_level: LogLevel) -> Self {
        self.args.push(CString::new("--log-level").unwrap());
        self.args
            .push(CString::new((log_level as u32).to_string()).unwrap());
        self
    }

    /// It calls `rte_eal_init` to initialize the Environment Abstraction Layer (EAL). This function
    /// is to be executed on the MAIN lcore only, as soon as possible in the application's main()
    /// function. It puts the WORKER lcores in the WAIT state.
    #[inline]
    #[allow(clippy::unwrap_in_result)]
    pub fn enter(self) -> Result<()> {
        #[allow(clippy::unwrap_used)]
        if CONTEXT.read().unwrap().is_some() {
            return Err(Error::Already);
        }
        let mut pargs = self
            .args
            .iter()
            .map(|s| s.as_ptr() as *mut c_char)
            .collect::<Vec<_>>();
        // SAFETY: ffi
        #[allow(unsafe_code)]
        let ret = unsafe { rte_eal_init(pargs.len() as _, pargs.as_mut_ptr()) };
        if ret < 0 {
            return Err(Error::from_errno());
        }
        let context = Arc::new(Eal {});
        #[allow(clippy::unwrap_used)]
        *CONTEXT.write().unwrap() = Some(context);
        net_dev::device_probe(self.addrs)?;
        Ok(())
    }
}
