#![no_std]

extern crate alloc;

use alloc::string::String;
use core::{fmt, ops::Deref};

mod bindings {
    wit_bindgen::generate!({
        world: "guest-support",
        path: "wit",
        generate_all,
    });
}

pub mod path {
    use super::*;

    /// A validated Cell guest path.
    ///
    /// Cell guest paths are UTF-8, relative, `/`-separated capability paths. They
    /// intentionally do not expose `std::path::Path` or `PathBuf`, because guest
    /// components should describe paths for the Cell filesystem capability rather
    /// than depend on host platform path semantics.
    #[repr(transparent)]
    #[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
    pub struct CellPath(str);

    /// An owned Cell guest path.
    #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
    pub struct CellPathBuf {
        inner: String,
    }

    /// Error returned when constructing a Cell guest path from an invalid string.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct InvalidPath {
        message: &'static str,
    }

    impl CellPath {
        /// Returns the root directory of the filesystem capability.
        pub fn root() -> &'static Self {
            Self::new(".").expect("'.' is a valid Cell path")
        }

        /// Validate and borrow a Cell path.
        pub fn new(path: &str) -> Result<&Self, InvalidPath> {
            validate(path)?;

            // SAFETY: CellPath is repr(transparent) over str and validate() has
            // checked the string follows Cell guest path rules.
            Ok(unsafe { &*(path as *const str as *const Self) })
        }

        /// Return the WIT string representation of this path.
        pub fn as_str(&self) -> &str {
            &self.0
        }

        /// Create an owned path from this borrowed path.
        pub fn to_cell_path_buf(&self) -> CellPathBuf {
            CellPathBuf {
                inner: String::from(self.as_str()),
            }
        }
    }

    impl CellPathBuf {
        /// Validate and allocate an owned Cell path.
        pub fn new(path: impl Into<String>) -> Result<Self, InvalidPath> {
            let path = path.into();
            validate(&path)?;
            Ok(Self { inner: path })
        }

        /// Returns the root directory of the filesystem capability.
        pub fn root() -> Self {
            Self {
                inner: String::from("."),
            }
        }

        pub fn as_cell_path(&self) -> &CellPath {
            CellPath::new(&self.inner).expect("CellPathBuf only stores validated paths")
        }

        pub fn as_str(&self) -> &str {
            &self.inner
        }

        pub fn into_string(self) -> String {
            self.inner
        }
    }

    impl Deref for CellPathBuf {
        type Target = CellPath;

        fn deref(&self) -> &Self::Target {
            self.as_cell_path()
        }
    }

    impl AsRef<CellPath> for CellPath {
        fn as_ref(&self) -> &CellPath {
            self
        }
    }

    impl AsRef<CellPath> for CellPathBuf {
        fn as_ref(&self) -> &CellPath {
            self.as_cell_path()
        }
    }

    impl fmt::Display for CellPath {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.as_str())
        }
    }

    impl fmt::Display for CellPathBuf {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.as_str())
        }
    }

    impl InvalidPath {
        pub fn message(&self) -> &'static str {
            self.message
        }
    }

    impl fmt::Display for InvalidPath {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.message)
        }
    }

    fn validate(path: &str) -> Result<(), InvalidPath> {
        if path.is_empty() {
            return Err(InvalidPath {
                message: "path must not be empty",
            });
        }

        if path == "." {
            return Ok(());
        }

        if path.starts_with('/') {
            return Err(InvalidPath {
                message: "absolute paths are not allowed",
            });
        }

        if path.contains('\\') {
            return Err(InvalidPath {
                message: "backslash path separators are not allowed",
            });
        }

        if path.contains('\0') {
            return Err(InvalidPath {
                message: "NUL bytes are not allowed in paths",
            });
        }

        for component in path.split('/') {
            match component {
                "" => {
                    return Err(InvalidPath {
                        message: "empty path components are not allowed",
                    });
                }
                "." => {
                    return Err(InvalidPath {
                        message: "current-directory components are not allowed",
                    });
                }
                ".." => {
                    return Err(InvalidPath {
                        message: "parent-directory components are not allowed",
                    });
                }
                _ => {}
            }
        }

        Ok(())
    }
}

mod error;
pub use error::{FsError, FsErrorKind};

pub mod fs {
    use alloc::{string::String, vec::Vec};

    use super::{bindings, path::CellPath};

    pub use crate::{FsError, FsErrorKind};
    pub use bindings::cell::fs::fs::{
        DirectoryEntry, FileType, Metadata, RevisionedText, SaveOutcome,
    };

    pub fn read_text(path: impl AsRef<CellPath>) -> Result<String, FsError> {
        bindings::cell::fs::fs::read_text(path.as_ref().as_str()).map_err(Into::into)
    }

    pub fn read_text_revision(path: impl AsRef<CellPath>) -> Result<RevisionedText, FsError> {
        bindings::cell::fs::fs::read_text_revision(path.as_ref().as_str()).map_err(Into::into)
    }

    pub fn save_text(
        path: impl AsRef<CellPath>,
        contents: &str,
        expected_revision: Option<&str>,
    ) -> Result<SaveOutcome, FsError> {
        bindings::cell::fs::fs::save_text(path.as_ref().as_str(), contents, expected_revision)
            .map_err(Into::into)
    }

    pub fn write_text(path: impl AsRef<CellPath>, contents: &str) -> Result<(), FsError> {
        bindings::cell::fs::fs::write_text(path.as_ref().as_str(), contents).map_err(Into::into)
    }

    pub fn exists(path: impl AsRef<CellPath>) -> bool {
        bindings::cell::fs::fs::exists(path.as_ref().as_str())
    }

    /// Inspect an entry without following its final symlink.
    pub fn symlink_metadata(path: impl AsRef<CellPath>) -> Result<Metadata, FsError> {
        bindings::cell::fs::fs::symlink_metadata(path.as_ref().as_str()).map_err(Into::into)
    }

    pub fn list_dir(path: impl AsRef<CellPath>) -> Result<Vec<DirectoryEntry>, FsError> {
        bindings::cell::fs::fs::list_dir(path.as_ref().as_str()).map_err(Into::into)
    }

    pub fn create_dir(path: impl AsRef<CellPath>) -> Result<(), FsError> {
        bindings::cell::fs::fs::create_dir(path.as_ref().as_str()).map_err(Into::into)
    }

    pub fn remove_file(path: impl AsRef<CellPath>) -> Result<(), FsError> {
        bindings::cell::fs::fs::remove_file(path.as_ref().as_str()).map_err(Into::into)
    }

    pub fn remove_dir(path: impl AsRef<CellPath>) -> Result<(), FsError> {
        bindings::cell::fs::fs::remove_dir(path.as_ref().as_str()).map_err(Into::into)
    }
}

pub use path::{CellPath, CellPathBuf, InvalidPath};

#[cfg(test)]
mod tests {
    use super::{CellPath, CellPathBuf};

    #[test]
    fn accepts_relative_capability_paths() {
        assert_eq!(CellPath::new("input.txt").unwrap().as_str(), "input.txt");
        assert_eq!(
            CellPath::new("dir/input.txt").unwrap().as_str(),
            "dir/input.txt"
        );
        assert_eq!(CellPath::root().as_str(), ".");
    }

    #[test]
    fn rejects_paths_with_host_or_escape_semantics() {
        assert!(CellPath::new("").is_err());
        assert!(CellPath::new("/tmp/input.txt").is_err());
        assert!(CellPath::new("../input.txt").is_err());
        assert!(CellPath::new("dir/../input.txt").is_err());
        assert!(CellPath::new("dir//input.txt").is_err());
        assert!(CellPath::new("dir\\input.txt").is_err());
    }

    #[test]
    fn owned_paths_are_validated_once() {
        let path = CellPathBuf::new("created/output.txt").unwrap();

        assert_eq!(path.as_cell_path().as_str(), "created/output.txt");
        assert_eq!(path.into_string(), "created/output.txt");
    }
}
