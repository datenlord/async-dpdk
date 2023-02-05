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
#[doc(hidden)]
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
    #[error("Protocol error")]
    Proto,
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
    #[error("Not exist")]
    NotExist,
}

#[doc(hidden)]
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
            libc::EPROTO => Error::Proto,
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
