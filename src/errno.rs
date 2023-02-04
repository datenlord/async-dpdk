//! DPDK defined error codes.

use dpdk_sys::{errno, rte_exit, rte_strerror};
use std::{
    ffi::{IntoStringError, NulError},
    net::AddrParseError,
    os::raw::c_int,
    sync::PoisonError,
};
use tokio::sync::{mpsc::error::SendError, oneshot::error::RecvError};

/// async-dpdk defined Result.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors from DPDK and rust.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, thiserror::Error)]
pub enum Error {
    /// Operation not permitted.
    #[error("Operation not permitted")]
    NoPerm,
    /// No such file or directory.
    #[error("No such file or directory")]
    NoEntry,
    /// No such process.
    #[error("No such process")]
    NoProc,
    /// Interrupted system calls.
    #[error("Interrupted system call")]
    Interrupted,
    /// Input/output error.
    #[error("Input/output error")]
    IoErr,
    /// Device not configured.
    #[error("Device not configured")]
    NotConfigured,
    /// Argument list too long.
    #[error("Argument list too long")]
    TooBig,
    /// Execute format error.
    #[error("Exec format error")]
    NoExec,
    /// Bad fd.
    #[error("Bad fd")]
    BadFd,
    /// Resource temporarily unavailable.
    #[error("Resource temporarily unavailable")]
    TempUnavail,
    /// Cannot allocate memory.
    #[error("Cannot allocate memory")]
    NoMem,
    /// Permission denied.
    #[error("Permission denied")]
    NoAccess,
    /// Bad address.
    #[error("Bad address")]
    BadAddress,
    /// Device or resource busy.
    #[error("Device or resource busy")]
    Busy,
    /// File exists.
    #[error("File exists")]
    Exists,
    /// Invalid cross device link.
    #[error("Invalid cross device link")]
    CrossDev,
    /// No suck device.
    #[error("No such device")]
    NoDev,
    /// Invalid argument.
    #[error("Invalid argument")]
    InvalidArg,
    /// No space left on device.
    #[error("No space left on device")]
    NoSpace,
    /// Broken pipe.
    #[error("Broken pipe")]
    BrokenPipe,
    /// Numerical result out of range.
    #[error("Numerical result out of range")]
    OutOfRange,
    /// Value too large for defined data types.
    #[error("Value too large for defined data type")]
    Overflow,
    /// Not supported.
    #[error("Not supported")]
    NotSupported,
    /// Operation already in progress.
    #[error("Operation already in progress")]
    Already,
    /// No buffer space available.
    #[error("No buffer space available")]
    NoBuf,
    /// Operation not allowed in secondary processes.
    #[error("Operation not allowed in secondary processes")]
    Secondary, // RTE defined
    /// Missing `rte_config`.
    #[error("Missing rte_config")]
    NoConfig, // RTE defined
    /// Unknown error.
    #[error("Unknown error")]
    Unknown,
    /// Lock poisoned.
    #[error("Lock poisoned")]
    Poisoned,
    /// Needed resource not started.
    #[error("Needed resource not started")]
    NotStart,
    /// Not exist.
    #[error("Not exist")]
    NotExist,
}

impl Error {
    /// Read error code on stacks.
    #[inline]
    #[must_use]
    pub fn from_errno() -> Error {
        // SAFETY: read mutable static variable
        #[allow(unsafe_code)]
        let errno = unsafe { errno!() };
        errno.into()
    }

    /// Convert DPDK returned error code to `async_dpdk` defined error code.
    #[inline]
    pub fn from_ret(errno: i32) -> Result<()> {
        let errno = errno.saturating_neg();
        match errno {
            e if e <= 0 => Ok(()),
            e => Err(e.into()),
        }
    }

    /// Function to terminate the application immediately, printing an error message and returning
    /// the exit code back to the shell.
    ///
    /// Call it only if the error code is checked.
    #[inline]
    #[allow(unsafe_code)]
    pub(crate) fn parse_err(errno: c_int) {
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

impl<T> From<PoisonError<T>> for Error {
    #[inline]
    fn from(_error: PoisonError<T>) -> Self {
        Error::Poisoned
    }
}

impl<T> From<SendError<T>> for Error {
    #[inline]
    fn from(_error: SendError<T>) -> Self {
        Error::BrokenPipe
    }
}

impl From<RecvError> for Error {
    #[inline]
    fn from(_error: RecvError) -> Self {
        Error::BrokenPipe
    }
}

impl From<NulError> for Error {
    #[inline]
    fn from(_error: NulError) -> Self {
        Error::InvalidArg
    }
}

impl From<AddrParseError> for Error {
    #[inline]
    fn from(_error: AddrParseError) -> Self {
        Error::InvalidArg
    }
}

impl From<IntoStringError> for Error {
    #[inline]
    fn from(_error: IntoStringError) -> Self {
        Error::InvalidArg
    }
}
