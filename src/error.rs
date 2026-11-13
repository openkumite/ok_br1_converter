//! The library's error type: what went wrong reading the disc or its data, or writing the output. The CLI wraps it
//! in `anyhow`; the library itself has no `anyhow`.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The disc image (or an output file) could not be read or written.
    Io(String),
    /// Data that is not what the converter expects (a truncated container, a bad TIM, …).
    Format(String),
    /// A readable disc, but not Bloody Roar (USA); `found` says what it is.
    WrongDisc { found: String },
    /// Raw 2352-byte sectors needed (the XA / STR streams of an ISO image).
    NotRaw2352(String),
    /// The output sink refused a file.
    Sink(String),
    /// The caller's progress callback asked to stop.
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "read error: {e}"),
            Error::Format(e) => write!(f, "{e}"),
            Error::WrongDisc { found } => write!(f, "this is not Bloody Roar (USA, SCUS-94199): {found}"),
            Error::NotRaw2352(e) => write!(f, "raw 2352-byte sectors needed: {e}"),
            Error::Sink(e) => write!(f, "could not write: {e}"),
            Error::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e.to_string())
    }
}

impl From<std::array::TryFromSliceError> for Error {
    fn from(e: std::array::TryFromSliceError) -> Error {
        Error::Format(e.to_string())
    }
}

impl From<std::num::ParseIntError> for Error {
    fn from(e: std::num::ParseIntError) -> Error {
        Error::Format(e.to_string())
    }
}

impl From<png::EncodingError> for Error {
    fn from(e: png::EncodingError) -> Error {
        Error::Format(format!("PNG: {e}"))
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::Format(format!("JSON: {e}"))
    }
}

/// `anyhow::Context` for the library's `Result` and for `Option`: the message goes in front of the error.
pub trait Context<T> {
    fn context<C: fmt::Display>(self, c: C) -> Result<T>;
    fn with_context<C: fmt::Display, F: FnOnce() -> C>(self, f: F) -> Result<T>;
}

impl<T, E: fmt::Display> Context<T> for std::result::Result<T, E> {
    fn context<C: fmt::Display>(self, c: C) -> Result<T> {
        self.map_err(|e| Error::Format(format!("{c}: {e}")))
    }
    fn with_context<C: fmt::Display, F: FnOnce() -> C>(self, f: F) -> Result<T> {
        self.map_err(|e| Error::Format(format!("{}: {e}", f())))
    }
}

impl<T> Context<T> for Option<T> {
    fn context<C: fmt::Display>(self, c: C) -> Result<T> {
        self.ok_or_else(|| Error::Format(c.to_string()))
    }
    fn with_context<C: fmt::Display, F: FnOnce() -> C>(self, f: F) -> Result<T> {
        self.ok_or_else(|| Error::Format(f().to_string()))
    }
}

/// `Error::Format` from a format string (`anyhow!`).
#[macro_export]
macro_rules! format_err {
    ($($t:tt)*) => {
        $crate::error::Error::Format(format!($($t)*))
    };
}

/// Returns `Err(Error::Format(..))` (`anyhow::bail!`).
#[macro_export]
macro_rules! bail {
    ($($t:tt)*) => {
        return Err($crate::format_err!($($t)*))
    };
}
