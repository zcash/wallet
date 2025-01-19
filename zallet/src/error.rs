use std::fmt;
use std::ops::Deref;

use abscissa_core::error::{BoxError, Context};

macro_rules! wfl {
    ($f:ident, $message_id:literal) => {
        write!($f, "{}", $crate::fl!($message_id))
    };

    ($f:ident, $message_id:literal, $($args:expr),* $(,)?) => {
        write!($f, "{}", $crate::fl!($message_id, $($args), *))
    };
}

macro_rules! wlnfl {
    ($f:ident, $message_id:literal) => {
        writeln!($f, "{}", $crate::fl!($message_id))
    };

    ($f:ident, $message_id:literal, $($args:expr),* $(,)?) => {
        writeln!($f, "{}", $crate::fl!($message_id, $($args), *))
    };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    Discombobulated,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::Discombobulated => wlnfl!(f, "err-discombobulated")?,
        }
        writeln!(f)?;
        writeln!(f, "[ {} ]", crate::fl!("err-ux-A"))?;
        write!(
            f,
            "[ {}: https://github.com/zcash/wallet/issues {} ]",
            crate::fl!("err-ux-B"),
            crate::fl!("err-ux-C")
        )
    }
}

impl std::error::Error for ErrorKind {}

impl ErrorKind {
    /// Creates an error context from this error.
    pub(crate) fn context(self, source: impl Into<BoxError>) -> Context<ErrorKind> {
        Context::new(self, Some(source.into()))
    }
}

/// Error type
#[derive(Debug)]
pub(crate) struct Error(Box<Context<ErrorKind>>);

impl Deref for Error {
    type Target = Context<ErrorKind>;

    fn deref(&self) -> &Context<ErrorKind> {
        &self.0
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Context::new(kind, None).into()
    }
}

impl From<Context<ErrorKind>> for Error {
    fn from(context: Context<ErrorKind>) -> Self {
        Error(Box::new(context))
    }
}
