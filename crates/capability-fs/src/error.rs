use std::fmt;

/// Stable categories; consumers must include a fallback arm.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FsErrorKind {
    PermissionDenied,
    NotFound,
    InvalidPath,
    ConfinementViolation,
    InvalidTextEncoding,
    ContentTooLarge,
    NonRegularFile,
    AlreadyExists,
    UnsupportedOperation,
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

impl FsError {
    pub(crate) fn new(kind: FsErrorKind, path: &str, operation: &str) -> Self {
        Self {
            kind,
            path: path.into(),
            operation: operation.into(),
        }
    }
    pub(crate) fn context(mut self, path: &str, operation: &str) -> Self {
        self.path = path.into();
        self.operation = operation.into();
        self
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:?}: {:?}", self.operation, self.path, self.kind)
    }
}
impl std::error::Error for FsError {}

impl From<FsError> for crate::bindings::cell::fs::fs::FsError {
    fn from(error: FsError) -> Self {
        use crate::bindings::cell::fs::fs::ErrorCategory as Wire;
        let category = match error.kind {
            FsErrorKind::PermissionDenied => Wire::PermissionDenied,
            FsErrorKind::NotFound => Wire::NotFound,
            FsErrorKind::InvalidPath => Wire::InvalidPath,
            FsErrorKind::ConfinementViolation => Wire::ConfinementViolation,
            FsErrorKind::InvalidTextEncoding => Wire::InvalidTextEncoding,
            FsErrorKind::ContentTooLarge => Wire::Other("content-too-large".into()),
            FsErrorKind::NonRegularFile => Wire::Other("non-regular-file".into()),
            FsErrorKind::AlreadyExists => Wire::Other("already-exists".into()),
            FsErrorKind::UnsupportedOperation => Wire::Other("unsupported-operation".into()),
            FsErrorKind::Io => Wire::Io,
            FsErrorKind::Other(value) => Wire::Other(value),
        };
        Self {
            category,
            path: error.path,
            operation: error.operation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_unknown_wire_category() {
        let error = FsError::new(
            FsErrorKind::Other("future-category".into()),
            "grant/file",
            "future-operation",
        );
        let wire: crate::bindings::cell::fs::fs::FsError = error.into();
        assert!(
            matches!(wire.category, crate::bindings::cell::fs::fs::ErrorCategory::Other(ref value) if value == "future-category")
        );
        assert_eq!(wire.path, "grant/file");
        assert_eq!(wire.operation, "future-operation");
    }
}
