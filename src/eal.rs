//! EAL (Environment Abstract Layer)

use crate::net_dev;
use crate::{Error, Result};
use dpdk_sys::*;
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
pub struct Eal {}

#[allow(unsafe_code)]
unsafe impl Sync for Eal {}

/// Check if a primary process is currently alive.
///
/// This function returns true when a primary process is currently active.
pub fn primary_proc_alive() -> bool {
    todo!()
}

/// Disable multiprocess.
///
/// This function can be called to indicate that multiprocess won't be used for the rest of
/// the application life.
#[allow(unsafe_code)]
pub fn disable_mp() -> bool {
    // SAFETY: ffi
    unsafe { rte_mp_disable() }
}

/// Whether EAL is using hugepages.
#[allow(unsafe_code)]
pub fn has_hugepages() -> bool {
    // SAFETY: ffi
    unsafe { rte_eal_has_hugepages() != 0 }
}

/// Whether EAL is using PCI bus. Disabled by –no-pci option.
#[allow(unsafe_code)]
pub fn has_pci() -> bool {
    // SAFETY: ffi
    unsafe { rte_eal_has_pci() != 0 }
}

/// Whether the EAL was asked to create UIO device.
#[allow(unsafe_code)]
pub fn uio_created() -> bool {
    // SAFETY: ffi
    unsafe { rte_eal_create_uio_dev() != 0 }
}

/// The user-configured vfio interrupt mode.
#[allow(unsafe_code)]
pub fn vfio_intr_mode() {
    todo!()
}

/// Get the runtime directory of DPDK
#[allow(unsafe_code)]
pub fn runtime_dir() -> PathBuf {
    // SAFETY: ffi
    let ptr = unsafe { rte_eal_get_runtime_dir() };
    // SAFETY: read C string
    let cs = unsafe { CString::from_raw(ptr as _) };
    PathBuf::from(cs.into_string().unwrap())
}

impl Drop for Eal {
    fn drop(&mut self) {
        // Close all devices
        net_dev::device_close().unwrap();
        // SAFETY: ffi
        #[allow(unsafe_code)]
        let errno = unsafe { rte_eal_cleanup() };
        Error::parse_err(errno);
    }
}

/// EAL Builder
#[derive(Debug, Default)]
pub struct Config {
    args: Vec<CString>,
    addrs: Vec<IpAddr>,
}

/// IOVA mode.
#[derive(Debug, Clone, Copy)]
pub enum IovaMode {
    /// physical address
    PA,
    /// virtual address
    VA,
}

/// DPDK log level.
#[repr(u32)]
#[derive(Debug, Clone, Copy)]
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
    pub fn new() -> Self {
        let env_args = std::env::args().collect::<Vec<_>>();
        Self {
            args: vec![CString::new(env_args[0].as_str()).unwrap()],
            addrs: vec![],
        }
    }

    /// Probe devices or not.
    pub fn device_probe(mut self, addr_str: &[&str]) -> Self {
        for addr in addr_str.iter() {
            self.addrs.push(IpAddr::from_str(addr).unwrap());
        }
        self
    }

    /// Set core mask to EAL.
    pub fn coremask(mut self, mask: u64) -> Self {
        self.args.push(CString::new("-c").unwrap());
        self.args.push(CString::new(mask.to_string()).unwrap());
        self
    }

    /// Set core list to EAL.
    pub fn corelist(mut self, list: &str) -> Self {
        self.args.push(CString::new("-l").unwrap());
        self.args.push(CString::new(list).unwrap());
        self
    }

    /// Set core map to EAL.
    pub fn coremap(mut self, map: &str) -> Self {
        self.args.push(CString::new("-lcores").unwrap());
        self.args.push(CString::new(map).unwrap());
        self
    }

    /// Set master lcore.
    pub fn master_core(mut self, id: u64) -> Self {
        self.args.push(CString::new("--master-lcore").unwrap());
        self.args.push(CString::new(id.to_string()).unwrap());
        self
    }

    /// Set service lcores.
    pub fn service_cores(mut self, mask: u64) -> Self {
        self.args.push(CString::new("-s").unwrap());
        self.args.push(CString::new(mask.to_string()).unwrap());
        self
    }

    /// Set pci blacklist.
    pub fn pci_blacklist(mut self, name: &str) -> Self {
        self.args.push(CString::new("-b").unwrap());
        self.args.push(CString::new(name).unwrap());
        self
    }

    /// Set pci whitelist.
    pub fn pci_whitelist(mut self, name: &str) -> Self {
        self.args.push(CString::new("-w").unwrap());
        self.args.push(CString::new(name).unwrap());
        self
    }

    /// Disable PCI.
    pub fn no_pci(mut self, no_pci: bool) -> Self {
        if no_pci {
            self.args.push(CString::new("--no-pci").unwrap());
        }
        self
    }

    /// Add a vdev.
    pub fn vdev(mut self, vdev: &str) -> Self {
        self.args.push(CString::new("--vdev").unwrap());
        self.args.push(CString::new(vdev).unwrap());
        self
    }

    /// Number of socket channels.
    pub fn num_channels(mut self, channels: u32) -> Self {
        self.args.push(CString::new("-n").unwrap());
        self.args.push(CString::new(channels.to_string()).unwrap());
        self
    }

    /// Number of memory ranks.
    pub fn num_ranks(mut self, ranks: u32) -> Self {
        self.args.push(CString::new("-r").unwrap());
        self.args.push(CString::new(ranks.to_string()).unwrap());
        self
    }

    /// Set in-memory.
    pub fn in_memory(mut self) -> Self {
        self.args.push(CString::new("--in-memory").unwrap());
        self
    }

    /// Reserved memory on start in megabytes.
    pub fn memory_mb(mut self, size: u32) -> Self {
        self.args.push(CString::new("-m").unwrap());
        self.args.push(CString::new(size.to_string()).unwrap());
        self
    }

    /// Set iova mode.
    pub fn iova_mode(mut self, mode: IovaMode) -> Self {
        self.args.push(CString::new("--iova-mode").unwrap());
        match mode {
            IovaMode::PA => self.args.push(CString::new("pa").unwrap()),
            IovaMode::VA => self.args.push(CString::new("va").unwrap()),
        }
        self
    }

    /// Set log level
    pub fn log_level(mut self, log_level: LogLevel) -> Self {
        self.args.push(CString::new("--log-level").unwrap());
        self.args
            .push(CString::new((log_level as u32).to_string()).unwrap());
        self
    }

    /// It calls `rte_eal_init` to initialize the Environment Abstraction Layer (EAL). This function
    /// is to be executed on the MAIN lcore only, as soon as possible in the application's main()
    /// function. It puts the WORKER lcores in the WAIT state.
    pub fn enter(self) -> Result<()> {
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
        *CONTEXT.write().unwrap() = Some(context.clone());
        net_dev::device_probe(self.addrs)?;
        Ok(())
    }
}
