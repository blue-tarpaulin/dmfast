//! [`Error`]: why a feature (or a step of one) can't install, as the text the driver prints.

use std::borrow::Cow;
use std::fmt;

/// Why something can't install; prints as its message.
#[derive(Clone, PartialEq, Eq)]
pub struct Error {
    msg: Cow<'static, str>,
}

/// [`Error`] by default.
pub type Result<T, E = Error> = core::result::Result<T, E>;

impl Error {
    /// The message.
    pub fn as_str(&self) -> &str {
        &self.msg
    }
}

impl From<&'static str> for Error {
    fn from(msg: &'static str) -> Error {
        Error { msg: Cow::Borrowed(msg) }
    }
}

impl From<String> for Error {
    fn from(msg: String) -> Error {
        Error { msg: Cow::Owned(msg) }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

/// As a `String`'s (quoted).
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.msg, f)
    }
}

impl std::error::Error for Error {}

/// `return Err(Error::from(format!(..)))`.
#[macro_export]
macro_rules! bail {
    ($($t:tt)*) => {
        return ::core::result::Result::Err($crate::Error::from(::std::format!($($t)*)))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_its_message() {
        fn f(n: u32) -> Result<()> {
            if n > 1 {
                crate::bail!("{n} matches");
            }
            Err("none".into())
        }
        assert_eq!(f(2).unwrap_err().to_string(), "2 matches");
        assert_eq!(format!("{:?}", f(0).unwrap_err()), format!("{:?}", "none"));
    }
}
