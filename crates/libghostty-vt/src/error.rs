//! Error handling.
use std::mem::MaybeUninit;

use crate::ffi;

/// Convenient alias for fallible return values from libghostty-vt.
pub type Result<T> = std::result::Result<T, Error>;

/// Possible errors libghostty-vt may return.
///
/// Upstream keeps adding result codes, so this is non-exhaustive.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum Error {
    /// Out of memory.
    OutOfMemory,
    /// Invalid value was specified or returned.
    InvalidValue,
    /// Ran out of space when writing to a buffer.
    OutOfSpace {
        /// Required minimum size of the buffer.
        required: usize,
    },
    /// Operation failed while reading from or writing to external I/O.
    IoError,
    /// Operation failed because encoded input exceeded a configured limit.
    LimitExceeded,
    /// Operation was rejected by a safety check (e.g. pasted text that could
    /// inject commands). Nothing was done. Confirm with the user and retry
    /// with the operation's allow flag set.
    Rejected,
    /// The option, key or feature is not available in this build of
    /// libghostty-vt (for example Kitty graphics compiled out), or a key is
    /// one this build does not know. Nothing was done.
    Unsupported,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfMemory => write!(f, "out of memory"),
            Self::InvalidValue => write!(f, "invalid value"),
            Self::OutOfSpace { required } => {
                write!(f, "out of space, {required} bytes required")
            }
            Self::IoError => write!(f, "external IO error"),
            Self::LimitExceeded => write!(f, "encoded input exceeded configured limit"),
            Self::Rejected => write!(f, "operation rejected"),
            Self::Unsupported => write!(f, "unsupported by this libghostty-vt build"),
        }
    }
}

impl std::error::Error for Error {}

/// Check, in debug builds, a call whose typed Rust inputs rule out every
/// documented failure. Setters on encoders and events return a result since
/// the ABI freeze, but fail only for a NULL handle, an unknown enum value, or a
/// feature compiled out of the library; a failure here means the linked
/// library was built without the feature, which tests should catch.
#[inline]
#[track_caller]
pub(crate) fn debug_expect_success(code: ffi::Result::Type, call: &str) {
    debug_assert_eq!(code, ffi::Result::SUCCESS, "{call} failed");
}

pub(crate) fn from_result(code: ffi::Result::Type) -> Result<()> {
    match code {
        ffi::Result::SUCCESS => Ok(()),
        ffi::Result::OUT_OF_MEMORY => Err(Error::OutOfMemory),
        ffi::Result::OUT_OF_SPACE => Err(Error::OutOfSpace { required: 0 }),
        ffi::Result::IO_ERROR => Err(Error::IoError),
        ffi::Result::REJECTED => Err(Error::Rejected),
        ffi::Result::LIMIT_EXCEEDED => Err(Error::LimitExceeded),
        ffi::Result::UNSUPPORTED => Err(Error::Unsupported),
        _ => Err(Error::InvalidValue),
    }
}

pub(crate) fn from_optional_result_uninit<T>(
    code: ffi::Result::Type,
    v: MaybeUninit<T>,
) -> Result<Option<T>> {
    match code {
        // SAFETY: Value should be initialized after successful call.
        ffi::Result::SUCCESS => Ok(Some(unsafe { v.assume_init() })),
        ffi::Result::OUT_OF_MEMORY => Err(Error::OutOfMemory),
        ffi::Result::OUT_OF_SPACE => Err(Error::OutOfSpace { required: 0 }),
        ffi::Result::NO_VALUE => Ok(None),
        ffi::Result::IO_ERROR => Err(Error::IoError),
        ffi::Result::REJECTED => Err(Error::Rejected),
        ffi::Result::LIMIT_EXCEEDED => Err(Error::LimitExceeded),
        ffi::Result::UNSUPPORTED => Err(Error::Unsupported),
        _ => Err(Error::InvalidValue),
    }
}

pub(crate) fn from_optional_result<T>(code: ffi::Result::Type, v: T) -> Result<Option<T>> {
    match code {
        // SAFETY: Value should be initialized after successful call.
        ffi::Result::SUCCESS => Ok(Some(v)),
        ffi::Result::OUT_OF_MEMORY => Err(Error::OutOfMemory),
        ffi::Result::OUT_OF_SPACE => Err(Error::OutOfSpace { required: 0 }),
        ffi::Result::NO_VALUE => Ok(None),
        ffi::Result::IO_ERROR => Err(Error::IoError),
        ffi::Result::REJECTED => Err(Error::Rejected),
        ffi::Result::LIMIT_EXCEEDED => Err(Error::LimitExceeded),
        ffi::Result::UNSUPPORTED => Err(Error::Unsupported),
        _ => Err(Error::InvalidValue),
    }
}

pub(crate) fn from_result_with_len(code: ffi::Result::Type, len: usize) -> Result<usize> {
    match code {
        ffi::Result::SUCCESS => Ok(len),
        ffi::Result::OUT_OF_MEMORY => Err(Error::OutOfMemory),
        ffi::Result::OUT_OF_SPACE => Err(Error::OutOfSpace { required: len }),
        ffi::Result::IO_ERROR => Err(Error::IoError),
        ffi::Result::REJECTED => Err(Error::Rejected),
        ffi::Result::LIMIT_EXCEEDED => Err(Error::LimitExceeded),
        ffi::Result::UNSUPPORTED => Err(Error::Unsupported),
        _ => Err(Error::InvalidValue),
    }
}

pub(crate) fn from_optional_result_with_len(
    code: ffi::Result::Type,
    len: usize,
) -> Result<Option<usize>> {
    match code {
        ffi::Result::SUCCESS => Ok(Some(len)),
        ffi::Result::OUT_OF_MEMORY => Err(Error::OutOfMemory),
        ffi::Result::OUT_OF_SPACE => Err(Error::OutOfSpace { required: len }),
        ffi::Result::NO_VALUE => Ok(None),
        ffi::Result::IO_ERROR => Err(Error::IoError),
        ffi::Result::REJECTED => Err(Error::Rejected),
        ffi::Result::LIMIT_EXCEEDED => Err(Error::LimitExceeded),
        ffi::Result::UNSUPPORTED => Err(Error::Unsupported),
        _ => Err(Error::InvalidValue),
    }
}
