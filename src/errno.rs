//! DPDK defined error numbers.
use crate::*;
use thiserror::Error;

#[allow(missing_docs)]
#[repr(i32)]
#[derive(Copy, Clone, Debug, Error)]
pub enum Error {
    #[error("Operation not permitted")]
    NoPerm = -libc::EPERM,
    #[error("No such file or directory")]
    NoEntry = -libc::ENOENT,
    #[error("No such process")]
    NoProc = -libc::ESRCH,
    #[error("Interrupted system call")]
    Interrupted = -libc::EINTR,
    #[error("Input/output error")]
    IoErr = -libc::EIO,
    #[error("No such device or address")]
    NoAddr = -libc::ENXIO,
    #[error("Argument list too long")]
    TooBig = -libc::E2BIG,
    #[error("Exec format error")]
    NoExec = -libc::ENOEXEC,
    #[error("Bad fd")]
    BadFd = -libc::EBADF,
    #[error("Resource temporarily unavailable")]
    TempUnavail = -libc::EAGAIN,
    #[error("Cannot allocate memory")]
    NoMem = -libc::ENOMEM,
    #[error("Permission denied")]
    NoAccess = -libc::EACCES,
    #[error("Bad address")]
    BadAddress = -libc::EFAULT,
    #[error("Device or resource busy")]
    Busy = -libc::EBUSY,
    #[error("File exists")]
    Exists = -libc::EEXIST,
    #[error("Invalid cross device link")]
    CrossDev = -libc::EXDEV,
    #[error("No such device")]
    NoDev = -libc::ENODEV,
    #[error("Invalid argument")]
    InvalidArg = -libc::EINVAL,
    #[error("No space left on device")]
    NoSpace = -libc::ENOSPC,
    #[error("Broken pipe")]
    BrokenPipe = -libc::EPIPE,
    #[error("Numerical result out of range")]
    OutOfRange = -libc::ERANGE,
    #[error("Not supported")]
    NotSupported = -libc::ENOTSUP,
    #[error("Not exist")]
    NotExist,
    #[error("Unknown error")]
    Unknown,
}

#[allow(missing_docs)]
impl Error {
    #[inline]
    pub fn from_errno(errno: i32) -> Result<(), Error> {
        let errno = -errno;
        match errno {
            0 => Ok(()),
            libc::EPERM => Err(Self::NoPerm),
            libc::ENOENT => Err(Self::NoEntry),
            libc::ESRCH => Err(Self::NoProc),
            libc::EINTR => Err(Self::Interrupted),
            libc::EIO => Err(Self::IoErr),
            libc::ENXIO => Err(Self::NoAddr),
            libc::E2BIG => Err(Self::TooBig),
            libc::ENOEXEC => Err(Self::NoExec),
            libc::EBADF => Err(Self::BadFd),
            libc::EAGAIN => Err(Self::TempUnavail),
            libc::ENOMEM => Err(Self::NoMem),
            libc::EACCES => Err(Self::NoAccess),
            libc::EFAULT => Err(Self::BadAddress),
            libc::EBUSY => Err(Self::Busy),
            libc::EEXIST => Err(Self::Exists),
            libc::EXDEV => Err(Self::CrossDev),
            libc::ENODEV => Err(Self::NoDev),
            libc::EINVAL => Err(Self::InvalidArg),
            libc::ENOSPC => Err(Self::NoSpace),
            libc::EPIPE => Err(Self::BrokenPipe),
            libc::ERANGE => Err(Self::OutOfRange),
            libc::ENOTSUP => Err(Self::NotSupported),
            e if e > 0 => Err(Self::Unknown),
            _ => unreachable!(),
        }
    }

    #[inline]
    #[allow(unsafe_code)]
    pub fn parse_err(errno: c_int) {
        if errno < 0 {
            unsafe {
                let msg = rte_strerror(errno);
                rte_exit(errno, msg);
            }
        }
    }
}
