use alloc::string::String;
use core::fmt;

/// Stable categories; consumers must include a fallback arm.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FsErrorKind {
    PermissionDenied,
    NotFound,
    InvalidPath,
    ConfinementViolation,
    InvalidTextEncoding,
    Io,
    Other(String),
}

/// A sanitized failure in the logical capability namespace.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FsError {
    pub kind: FsErrorKind,
    pub path: String,
    pub operation: String,
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?}: {:?}", self.operation, self.path, self.kind)
    }
}
impl core::error::Error for FsError {}

impl From<crate::bindings::cell::fs::fs::FsError> for FsError {
    fn from(error: crate::bindings::cell::fs::fs::FsError) -> Self {
        use crate::bindings::cell::fs::fs::ErrorCategory as Wire;
        let kind = match error.category {
            Wire::PermissionDenied => FsErrorKind::PermissionDenied,
            Wire::NotFound => FsErrorKind::NotFound,
            Wire::InvalidPath => FsErrorKind::InvalidPath,
            Wire::ConfinementViolation => FsErrorKind::ConfinementViolation,
            Wire::InvalidTextEncoding => FsErrorKind::InvalidTextEncoding,
            Wire::Io => FsErrorKind::Io,
            Wire::Other(value) => FsErrorKind::Other(value),
        };
        Self {
            kind,
            path: error.path,
            operation: error.operation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decodes_every_category_and_preserves_context() {
        use crate::bindings::cell::fs::fs::{ErrorCategory as Wire, FsError as WireError};
        for (category, kind) in [
            (Wire::PermissionDenied, FsErrorKind::PermissionDenied),
            (Wire::NotFound, FsErrorKind::NotFound),
            (Wire::InvalidPath, FsErrorKind::InvalidPath),
            (
                Wire::ConfinementViolation,
                FsErrorKind::ConfinementViolation,
            ),
            (Wire::InvalidTextEncoding, FsErrorKind::InvalidTextEncoding),
            (Wire::Io, FsErrorKind::Io),
            (
                Wire::Other("future".into()),
                FsErrorKind::Other("future".into()),
            ),
        ] {
            let error: FsError = WireError {
                category,
                path: "grant/file".into(),
                operation: "read-text".into(),
            }
            .into();
            assert_eq!(error.kind, kind);
            assert_eq!(error.path, "grant/file");
            assert_eq!(error.operation, "read-text");
        }
    }
}
