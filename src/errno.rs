//! DPDK defined error codes.

use dpdk_sys::{rte_errno_stub, rte_exit, rte_strerror};
use std::{
    ffi::{IntoStringError, NulError},
    net::AddrParseError,
    num::TryFromIntError,
    os::raw::c_int,
    sync::{mpsc::RecvError as StdRecvError, mpsc::SendError as StdSendError, PoisonError},
};
use tokio::sync::{
    mpsc::error::SendError as TokioMpscSendError,
    mpsc::error::TrySendError as TokioMpscTrySendError,
    oneshot::error::RecvError as TokioOneshotRecvError,
};

/// async-dpdk defined Result.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors from DPDK and rust.
#[doc(hidden)]
#[non_exhaustive]
#[repr(i32)]
#[derive(Copy, Clone, Debug, thiserror::Error)]
pub enum Error {
    #[error("Operation not permitted")]
    NoPerm = libc::EPERM,
    #[error("No such file or directory")]
    NoEntry = libc::ENOENT,
    #[error("No such process")]
    NoProc = libc::ESRCH,
    #[error("Interrupted system call")]
    Interrupted = libc::EINTR,
    #[error("Input/output error")]
    IoErr = libc::EIO,
    #[error("Device not configured")]
    NotConfigured = libc::ENXIO,
    #[error("Argument list too long")]
    TooBig = libc::E2BIG,
    #[error("Exec format error")]
    NoExec = libc::ENOEXEC,
    #[error("Bad fd")]
    BadFd = libc::EBADF,
    #[error("Resource temporarily unavailable")]
    TempUnavail = libc::EAGAIN,
    #[error("Cannot allocate memory")]
    NoMem = libc::ENOMEM,
    #[error("Permission denied")]
    NoAccess = libc::EACCES,
    #[error("Bad address")]
    BadAddress = libc::EFAULT,
    #[error("Device or resource busy")]
    Busy = libc::EBUSY,
    #[error("File exists")]
    Exists = libc::EEXIST,
    #[error("Invalid cross device link")]
    CrossDev = libc::EXDEV,
    #[error("No such device")]
    NoDev = libc::ENODEV,
    #[error("Invalid argument")]
    InvalidArg = libc::EINVAL,
    #[error("No space left on device")]
    NoSpace = libc::ENOSPC,
    #[error("Broken pipe")]
    BrokenPipe = libc::EPIPE,
    #[error("Numerical result out of range")]
    OutOfRange = libc::ERANGE,
    #[error("Value too large for defined data type")]
    Overflow = libc::EOVERFLOW,
    #[error("Not supported")]
    NotSupported = libc::ENOTSUP,
    #[error("Operation already in progress")]
    Already = libc::EALREADY,
    #[error("No buffer space available")]
    NoBuf = libc::ENOBUFS,
    #[error("Protocol error")]
    Proto = libc::EPROTO,
    #[error("Operation not allowed in secondary processes")]
    Secondary = 1001, // RTE defined
    #[error("Missing rte_config")]
    NoConfig = 1002, // RTE defined
    #[error("Lock poisoned")]
    Poisoned = 1003,
    #[error("Needed resource not started")]
    NotStart = 1004,
    #[error("Not exist")]
    NotExist = 1005,
    #[error("Unknown error")]
    Unknown,
}

#[doc(hidden)]
impl Error {
    /// Read error code on stacks.
    #[inline]
    #[must_use]
    pub fn from_errno() -> Error {
        // SAFETY: read mutable static variable
        #[allow(unsafe_code)]
        let errno = unsafe { rte_errno_stub() };
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
            1003 => Error::Poisoned,
            1004 => Error::NotStart,
            1005 => Error::NotExist,
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

impl<T> From<TokioMpscSendError<T>> for Error {
    #[inline]
    fn from(_error: TokioMpscSendError<T>) -> Self {
        Error::BrokenPipe
    }
}

impl<T> From<TokioMpscTrySendError<T>> for Error {
    #[inline]
    fn from(_error: TokioMpscTrySendError<T>) -> Self {
        Error::TempUnavail
    }
}

impl From<TokioOneshotRecvError> for Error {
    #[inline]
    fn from(_error: TokioOneshotRecvError) -> Self {
        Error::BrokenPipe
    }
}

impl<T> From<StdSendError<T>> for Error {
    #[inline]
    fn from(_error: StdSendError<T>) -> Self {
        Error::BrokenPipe
    }
}

impl From<StdRecvError> for Error {
    #[inline]
    fn from(_error: StdRecvError) -> Self {
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

impl From<TryFromIntError> for Error {
    #[inline]
    fn from(_error: TryFromIntError) -> Self {
        Error::InvalidArg
    }
}
