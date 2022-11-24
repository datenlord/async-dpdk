//! DPDK defined error numbers.
use dpdk_sys::{errno, rte_exit, rte_strerror};
use std::os::raw::c_int;

/// async-dpdk defined Result.
pub type Result<T> = std::result::Result<T, Error>;

#[allow(missing_docs, clippy::missing_docs_in_private_items)]
#[non_exhaustive]
#[derive(Copy, Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("Operation not permitted")]
    NoPerm,
    #[error("No such file or directory")]
    NoEntry,
    #[error("No such process")]
    NoProc,
    #[error("Interrupted system call")]
    Interrupted,
    #[error("Input/output error")]
    IoErr,
    #[error("Device not configured")]
    NotConfigured,
    #[error("Argument list too long")]
    TooBig,
    #[error("Exec format error")]
    NoExec,
    #[error("Bad fd")]
    BadFd,
    #[error("Resource temporarily unavailable")]
    TempUnavail,
    #[error("Cannot allocate memory")]
    NoMem,
    #[error("Permission denied")]
    NoAccess,
    #[error("Bad address")]
    BadAddress,
    #[error("Device or resource busy")]
    Busy,
    #[error("File exists")]
    Exists,
    #[error("Invalid cross device link")]
    CrossDev,
    #[error("No such device")]
    NoDev,
    #[error("Invalid argument")]
    InvalidArg,
    #[error("No space left on device")]
    NoSpace,
    #[error("Broken pipe")]
    BrokenPipe,
    #[error("Numerical result out of range")]
    OutOfRange,
    #[error("Value too large for defined data type")]
    Overflow,
    #[error("Not supported")]
    NotSupported,
    #[error("Operation already in progress")]
    Already,
    #[error("No buffer space available")]
    NoBuf,
    #[error("Operation not allowed in secondary processes")]
    Secondary, // RTE defined
    #[error("Missing rte_config")]
    NoConfig, // RTE defined
    #[error("Unknown error")]
    Unknown,
    #[error("Lock poisoned")]
    Poisoned,
    #[error("Needed resource not started")]
    NotStart,
}

#[allow(missing_docs, clippy::missing_docs_in_private_items)]
impl Error {
    #[inline]
    #[allow(clippy::must_use_candidate)]
    pub fn from_errno() -> Error {
        // SAFETY: read mutable static variable
        #[allow(unsafe_code)]
        let errno = unsafe { errno!() };
        errno.into()
    }

    #[inline]
    pub fn from_ret(errno: i32) -> Result<()> {
        #[allow(clippy::integer_arithmetic)]
        let errno = -errno;
        match errno {
            e if e <= 0 => Ok(()),
            e => Err(e.into()),
        }
    }

    #[inline]
    #[allow(unsafe_code)]
    pub fn parse_err(errno: c_int) {
        if errno < 0 {
            // SAFETY: ffi
            unsafe {
                let msg = rte_strerror(errno);
                rte_exit(errno, msg);
            }
        }
    }
}

impl From<i32> for Error {
    #[inline]
    fn from(errno: i32) -> Self {
        match errno {
            libc::EPERM => Error::NoPerm,
            libc::ENOENT => Error::NoEntry,
            libc::ESRCH => Error::NoProc,
            libc::EINTR => Error::Interrupted,
            libc::EIO => Error::IoErr,
            libc::ENXIO => Error::NotConfigured,
            libc::E2BIG => Error::TooBig,
            libc::ENOEXEC => Error::NoExec,
            libc::EBADF => Error::BadFd,
            libc::EAGAIN => Error::TempUnavail,
            libc::ENOMEM => Error::NoMem,
            libc::EACCES => Error::NoAccess,
            libc::EFAULT => Error::BadAddress,
            libc::EBUSY => Error::Busy,
            libc::EEXIST => Error::Exists,
            libc::EXDEV => Error::CrossDev,
            libc::ENODEV => Error::NoDev,
            libc::EINVAL => Error::InvalidArg,
            libc::ENOSPC => Error::NoSpace,
            libc::EPIPE => Error::BrokenPipe,
            libc::ERANGE => Error::OutOfRange,
            libc::EOVERFLOW => Error::Overflow,
            libc::ENOTSUP => Error::NotSupported,
            libc::EALREADY => Error::Already,
            libc::ENOBUFS => Error::NoBuf,
            1001 => Error::Secondary,
            1002 => Error::NoConfig,
            e if e > 0 => Error::Unknown,
            _ => unreachable!("errno = {}", errno), // negative number
        }
    }
}
