//! EAL (Environment Abstract Layer)

use crate::*;

/// EAL
#[derive(Debug)]
pub struct Eal;

impl Drop for Eal {
    fn drop(&mut self) {
        #[allow(unsafe_code)]
        let _ = unsafe { rte_eal_cleanup() };
    }
}

/// EAL Builder
#[derive(Debug)]
pub struct Builder {
    args: Vec<CString>,
}

impl Builder {
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
    /// Reserved memory on start in megabytes.
    pub fn memory_mb(mut self, size: u32) -> Self {
        self.args.push(CString::new("-m").unwrap());
        self.args.push(CString::new(size.to_string()).unwrap());
        self
    }
    /// Get an EAL instance.
    pub fn build(self) -> Eal {
        let mut args = self
            .args
            .iter()
            .map(|s| s.as_ptr() as *mut c_char)
            .collect::<Vec<_>>();
        #[allow(unsafe_code)]
        let _errno = unsafe { rte_eal_init(args.len() as _, args.as_mut_ptr()) };
        Eal
    }
}
