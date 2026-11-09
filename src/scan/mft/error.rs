use std::fmt;
use std::io;

/// A structural problem in on-disk data. Cheap to create on the hot parsing path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError(pub &'static str);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Shorthand for turning a failed bounds check into a [`ParseError`].
pub(crate) trait OrCorrupt<T> {
    fn or_corrupt(self, what: &'static str) -> Result<T, ParseError>;
}

impl<T> OrCorrupt<T> for Option<T> {
    #[inline]
    fn or_corrupt(self, what: &'static str) -> Result<T, ParseError> {
        self.ok_or(ParseError(what))
    }
}

/// Why an MFT scan could not be completed.
#[derive(Debug)]
pub enum MftError {
    /// The target is not on a local NTFS volume with a drive letter.
    NotLocalNtfs(String),
    /// Opening the raw volume was refused (the process is not elevated).
    AccessDenied,
    Io(io::Error),
    Corrupt(&'static str),
    /// The target folder could not be located in the MFT (e.g. created since the last flush).
    NotFound(String),
    Unsupported(&'static str),
    Cancelled,
}

impl MftError {
    /// Whether `Method::Auto` should fall back to the directory walker.
    pub fn should_fallback(&self) -> bool {
        !matches!(self, MftError::Cancelled)
    }
}

impl fmt::Display for MftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MftError::NotLocalNtfs(why) => write!(f, "not a local NTFS volume ({why})"),
            MftError::AccessDenied => f.write_str("not elevated (run as administrator for the fast MFT scan)"),
            MftError::Io(e) => write!(f, "I/O error reading the volume: {e}"),
            MftError::Corrupt(what) => write!(f, "unexpected on-disk data: {what}"),
            MftError::NotFound(what) => write!(f, "{what}"),
            MftError::Unsupported(what) => write!(f, "unsupported: {what}"),
            MftError::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for MftError {}

impl From<ParseError> for MftError {
    fn from(e: ParseError) -> Self {
        MftError::Corrupt(e.0)
    }
}

impl From<io::Error> for MftError {
    fn from(e: io::Error) -> Self {
        MftError::Io(e)
    }
}
