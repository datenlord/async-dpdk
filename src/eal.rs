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
//! # Examples
//!
//! A simple example of setting up the environment:
//!
//! ```no_run
//! use async_dpdk::eal::{self, IovaMode};
//!
//! eal::Config::new()
//!     .coremask(0x3f)
//!     .device_probe(&["192.168.0.1", "192.168.0.2"])
//!     .unwrap()
//!     .iova_mode(IovaMode::VA)
//!     .enter()
//!     .unwrap();
//! ```

use crate::{net_dev, Error, Result};
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

/// Create a new `CString`.
///
/// This macro is for internal use. `CString::new` returns an error if the passed-in
/// string is empty. DO NOT use it without checking.
macro_rules! cstring {
    ($s:expr) => {
        ::std::ffi::CString::new($s).unwrap()
    };
}

/// The existence of an `Eal` instance shows the readiness of a DPDK environment.
#[non_exhaustive]
#[derive(Debug)]
pub struct Eal {}

#[allow(unsafe_code)]
unsafe impl Sync for Eal {}

impl Drop for Eal {
    #[inline]
    fn drop(&mut self) {
        // Close all devices
        #[allow(clippy::unwrap_used)] // used in drop
        net_dev::device_close().unwrap();
        // SAFETY: ffi
        #[allow(unsafe_code)]
        let errno = unsafe { rte_eal_cleanup() };
        if let Err(e) = Error::from_ret(errno) {
            log::error!("Fatal error occurred in EAL cleanup: {e:?}");
        }
        Error::parse_err(errno);
    }
}

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

/// Get the runtime directory of DPDK.
///
/// # Errors
///
/// This function returns an error if the C string failed to convert to Rust string.
#[allow(unsafe_code)]
#[inline]
pub fn runtime_dir() -> Result<PathBuf> {
    // SAFETY: runtime dir pointer checked later
    let ptr = unsafe { rte_eal_get_runtime_dir() };
    if ptr.is_null() {
        return Err(Error::NotSupported);
    }
    // SAFETY: read C string
    let cs = unsafe { CString::from_raw(ptr as _) };
    Ok(PathBuf::from(cs.into_string().map_err(Error::from)?))
}

/// Used for the configuration of EAL.
#[derive(Debug, Default)]
pub struct Config {
    /// Args passed to `rte_eal_init`.
    args: Vec<CString>,
    /// IP addresses for each `EthDev`s.
    addrs: Vec<IpAddr>,
    /// Max RX/TX queues number for each devices.
    max_queues: Option<u16>,
}

/// IOVA mode. The addresses used by hardwares, it should either be physical addresses or
/// virtual addresses.
#[derive(Debug, Clone, Copy)]
#[allow(clippy::exhaustive_enums)]
pub enum IovaMode {
    /// physical address
    PA,
    /// virtual address
    VA,
}

/// DPDK-supported virtual device.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum Vdev {
    /// `Null` device is a simple virtual driver mainly for testing. It always
    /// returns success for all packets for Rx/Tx.
    ///
    /// Each `Null` device needs an unique integer as its id.
    ///
    /// On Rx it returns requested number of empty packets (all zero). On Tx it
    /// just frees all sent packets.
    Null(i32),

    /// 'Pcap' device allows you to read/write from/to a .pcap file.
    ///
    /// Each `Pcap` device needs an unique integer as its id.
    ///
    /// For more information, please refer to [`pcap_ring docs`].
    Pcap(i32),

    /// `Ring` device uses an `rte_ring` to emulate an Ethernet port. On Rx it gets
    /// a packet from the ring. On Tx it puts the packet to the ring.
    ///
    /// Each `Ring` device needs an unique integer as its id.
    ///
    /// For more information, please refer to [`pcap_ring docs`].
    ///
    /// [`pcap_ring docs`]: https://doc.dpdk.org/guides/nics/pcap_ring.html
    Ring(i32),
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
    /// Create a new eal `Config` instance.
    #[inline]
    #[must_use]
    #[allow(clippy::indexing_slicing)] // the first of env args is the program name
    pub fn new() -> Self {
        let env_args = std::env::args().collect::<Vec<_>>();
        Self {
            args: vec![cstring!(env_args[0].as_str())],
            ..Default::default()
        }
    }

    /// Probe UIO/VFIO devices.
    ///
    /// This function takes a list of IP addresses as identifiers for each net device. The number
    /// of addresses should be less than the number of UIO/VFIO devices. The devices will be started
    /// after entering EAL.
    ///
    /// # Errors
    ///
    /// The function returns an error if the address strings does not match IP format.
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
    pub fn coremask(mut self, mask: u64) -> Self {
        self.args.push(cstring!("-c"));
        self.args.push(cstring!(mask.to_string()));
        self
    }

    /// Set core list to EAL.
    ///
    /// # Errors
    ///
    /// The function returns an error if the `list` argument is empty.
    #[inline]
    pub fn corelist(mut self, list: &str) -> Result<Self> {
        self.args.push(CString::new("-l").map_err(Error::from)?);
        self.args.push(CString::new(list).map_err(Error::from)?);
        Ok(self)
    }

    /// Set core map to EAL.
    ///
    /// # Errors
    ///
    /// The function returns an error if the `map` argument is empty.
    #[inline]
    pub fn coremap(mut self, map: &str) -> Result<Self> {
        self.args
            .push(CString::new("-lcores").map_err(Error::from)?);
        self.args.push(CString::new(map).map_err(Error::from)?);
        Ok(self)
    }

    /// Set pci blacklist.
    ///
    /// # Errors
    ///
    /// The function returns an error if the `name` argument is empty.
    #[inline]
    pub fn pci_blacklist(mut self, name: &str) -> Result<Self> {
        self.args.push(CString::new("-b").map_err(Error::from)?);
        self.args.push(CString::new(name).map_err(Error::from)?);
        Ok(self)
    }

    /// Set pci whitelist.
    ///
    /// # Errors
    ///
    /// The function returns an error if the `name` argument is empty.
    #[inline]
    pub fn pci_whitelist(mut self, name: &str) -> Result<Self> {
        self.args.push(CString::new("-w").map_err(Error::from)?);
        self.args.push(CString::new(name).map_err(Error::from)?);
        Ok(self)
    }

    /// Disable PCI.
    #[inline]
    #[must_use]
    pub fn no_pci(mut self, no_pci: bool) -> Self {
        if no_pci {
            self.args.push(cstring!("--no-pci"));
        }
        self
    }

    /// Disable hugepages. Required to run without root.
    #[inline]
    #[must_use]
    #[allow(clippy::unwrap_used, clippy::missing_panics_doc)] // impossible to panic
    pub fn no_hugepages(mut self, no_huge: bool) -> Self {
        if no_huge {
            self.args.push(CString::new("--no-pci").unwrap());
        }
        self
    }

    /// Reserved memory on start in megabytes.
    #[inline]
    #[must_use]
    pub fn memory_mb(mut self, size: u32) -> Self {
        self.args.push(cstring!("-m"));
        self.args.push(cstring!(size.to_string()));
        self
    }

    /// Set iova mode.
    #[inline]
    #[must_use]
    pub fn iova_mode(mut self, mode: IovaMode) -> Self {
        self.args.push(cstring!("--iova-mode"));
        match mode {
            IovaMode::PA => self.args.push(cstring!("pa")),
            IovaMode::VA => self.args.push(cstring!("va")),
        }
        self
    }

    /// Add a virtual device.
    #[inline]
    #[must_use]
    pub fn vdev(mut self, vdev: Vdev) -> Self {
        self.args.push(cstring!("--vdev"));
        match vdev {
            Vdev::Null(id) => self.args.push(cstring!(format!("net_null{id}"))),
            Vdev::Pcap(id) => self.args.push(cstring!(format!("net_pcap{id}"))),
            Vdev::Ring(id) => self.args.push(cstring!(format!("net_ring{id}"))),
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

    /// Set max RX/TX queue number.
    #[inline]
    #[must_use]
    pub fn max_queues(mut self, max_queues: u16) -> Self {
        self.max_queues = Some(max_queues);
        self
    }

    /// Initialize the Environment Abstraction Layer (EAL). This function is to be executed on the MAIN
    /// lcore only, as soon as possible in the application's `main()` function.
    ///
    /// # Errors
    ///
    /// Possible reasons for failure:
    ///
    /// - `Error::NoAccess` indicates a permissions issue.
    /// - `Error::TempUnavail` indicates either a bus or system resource was not available, setup may be
    ///   attempted again.
    /// - `Error::Already` indicates that the EAL has already been initialized, and cannot be initialized
    ///   again.
    /// - `Error::InvalidArg` indicates invalid parameters were passed.
    /// - `Error::NoMem` indicates failure likely caused by an out-of-memory condition.
    /// - `Error::NoDev` indicates memory setup issues.
    /// - `Error::NotSupported` indicates that the EAL cannot initialize on this system.
    /// - `Error::Proto` indicates that the PCI bus is either not present, or is not readable by the eal.
    /// - `Error::NoExec` indicates that a service core failed to launch successfully.
    /// - `Error::ToBig` indicates that there are too many configuration items.
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
            // arg length checked
            rte_eal_init(
                pargs.len().try_into().map_err(Error::from)?,
                pargs.as_mut_ptr(),
            )
        };
        if ret < 0 {
            error!("Error initializing DPDK environment");
            return Err(Error::from_errno());
        }
        let context = Arc::new(Eal {});
        *CONTEXT.write().map_err(Error::from)? = Some(context);
        if let Some(max_queues) = self.max_queues {
            if max_queues == 0 {
                return Err(Error::InvalidArg);
            }
        }
        net_dev::device_probe(self.addrs, self.max_queues.unwrap_or(u16::MAX))?;
        Ok(())
    }
}
