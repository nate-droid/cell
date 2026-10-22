mod error;
mod watch;
pub use error::{FsError, FsErrorKind};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};
pub use watch::{FsWatch, RescanReason, WatchEvent, WatchOptions};

use wasmtime::component::Linker;

pub use bindings::cell::fs::fs::{DirectoryEntry, FileType, Metadata, RevisionedText, SaveOutcome};
use cap_std::{ambient_authority, fs::Dir};

#[derive(Debug)]
struct Root {
    path: PathBuf,
    dir: Dir,
}

impl Root {
    fn open(path: impl Into<PathBuf>) -> Result<Self, FsError> {
        let path = path
            .into()
            .canonicalize()
            .map_err(|e| io_error(e, ".", "open-root"))?;
        let dir = Dir::open_ambient_dir(&path, ambient_authority())
            .map_err(|e| io_error(e, ".", "open-root"))?;
        Ok(Self { path, dir })
    }
}

// Mount equality describes the host's configuration, not handle identity.
impl PartialEq for Root {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}
impl Eq for Root {}

mod bindings {
    wasmtime::component::bindgen!({
        world: "host",
        path: "wit",
    });
}

/// A filesystem capability rooted at a host directory.
///
/// Guests can only address relative paths beneath the configured root. Absolute
/// paths, parent-directory traversal, and Windows path prefixes are rejected.
/// Operations resolve through a retained directory handle. Relative symlinks are
/// followed only within the grant; absolute symlink targets are rejected.
/// `symlink_metadata` and directory entry types do not follow final symlinks.
/// See the crate README for the confinement contract and host namespace limits.
#[derive(Clone, Debug)]
pub struct LocalFs {
    root: Arc<Root>,
    writable: bool,
    logical_name: String,
}

impl LocalFs {
    /// Set a display-only logical root name. Never supply secrets or host paths.
    /// This does not change guest path resolution or authority.
    pub fn with_logical_name(mut self, name: impl Into<String>) -> Self {
        self.logical_name = name.into();
        self
    }

    /// Create a readable and writable filesystem rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, FsError> {
        Self::with_permissions(root, true)
    }

    fn with_permissions(root: impl Into<PathBuf>, writable: bool) -> Result<Self, FsError> {
        let root = Root::open(root)?;

        Ok(Self {
            root: Arc::new(root),
            writable,
            logical_name: "/".to_owned(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root.path
    }

    pub fn read_text(&self, path: impl AsRef<str>) -> Result<String, FsError> {
        read_text_at(&self.root, path.as_ref())
    }

    pub fn write_text(
        &self,
        path: impl AsRef<str>,
        contents: impl AsRef<str>,
    ) -> Result<(), FsError> {
        if !self.writable {
            return Err(FsError::new(
                FsErrorKind::PermissionDenied,
                path.as_ref(),
                "write-text",
            ));
        }

        write_text_at(&self.root, path.as_ref(), contents.as_ref())
    }

    /// Read a regular file with bounded buffering and a content revision.
    /// Follows confined symlinks; this is not an immutable snapshot.
    pub fn read_text_revision_bounded(
        &self,
        path: impl AsRef<str>,
        max_bytes: usize,
    ) -> Result<RevisionedText, FsError> {
        read_text_revision_bounded_at(&self.root, path.as_ref(), max_bytes)
    }

    /// Read text and its opaque content token in one read.
    pub fn read_text_revision(&self, path: impl AsRef<str>) -> Result<RevisionedText, FsError> {
        let contents = self.read_text(path)?;
        Ok(RevisionedText {
            revision: revision(contents.as_bytes()),
            contents,
        })
    }

    /// Exclusively publish complete text at a new entry and return its revision.
    ///
    /// Unix only; no fsync/durability guarantee is made. Existing entries,
    /// including dangling symlinks, are never replaced. The parent must not
    /// have hostile writers: a writer can replace the named temporary source
    /// or mutate its inode before publication, invalidating content/revision
    /// guarantees. A retained parent handle does not prevent these attacks.
    pub fn create_text(
        &self,
        path: impl AsRef<str>,
        contents: impl AsRef<str>,
    ) -> Result<String, FsError> {
        let path = path.as_ref();
        if !self.writable {
            return Err(FsError::new(
                FsErrorKind::PermissionDenied,
                path,
                "create-text",
            ));
        }
        create_text_at(&self.root, path, contents.as_ref())
    }

    /// Replace an existing regular file. See the crate README for platform,
    /// symlink, permission, optimistic conflict and durability guarantees.
    pub fn save_text(
        &self,
        path: impl AsRef<str>,
        contents: impl AsRef<str>,
        expected_revision: Option<&str>,
    ) -> Result<SaveOutcome, FsError> {
        let path = path.as_ref();
        if !self.writable {
            return Err(FsError::new(
                FsErrorKind::PermissionDenied,
                path,
                "save-text",
            ));
        }
        save_text_at(&self.root, path, contents.as_ref(), expected_revision)
    }

    pub fn exists(&self, path: impl AsRef<str>) -> bool {
        exists_at(&self.root, path.as_ref())
    }

    /// Inspect an entry without following its final symlink.
    pub fn symlink_metadata(&self, path: impl AsRef<str>) -> Result<Metadata, FsError> {
        symlink_metadata_at(&self.root, path.as_ref())
    }

    pub fn list_dir(&self, path: impl AsRef<str>) -> Result<Vec<DirectoryEntry>, FsError> {
        list_dir_at(&self.root, path.as_ref())
    }

    pub fn create_dir(&self, path: impl AsRef<str>) -> Result<(), FsError> {
        if !self.writable {
            return Err(FsError::new(
                FsErrorKind::PermissionDenied,
                path.as_ref(),
                "create-dir",
            ));
        }

        create_dir_at(&self.root, path.as_ref())
    }

    pub fn remove_file(&self, path: impl AsRef<str>) -> Result<(), FsError> {
        if !self.writable {
            return Err(FsError::new(
                FsErrorKind::PermissionDenied,
                path.as_ref(),
                "remove-file",
            ));
        }

        remove_file_at(&self.root, path.as_ref())
    }

    pub fn remove_dir(&self, path: impl AsRef<str>) -> Result<(), FsError> {
        if !self.writable {
            return Err(FsError::new(
                FsErrorKind::PermissionDenied,
                path.as_ref(),
                "remove-dir",
            ));
        }

        remove_dir_at(&self.root, path.as_ref())
    }
}

/// A read-only filesystem capability rooted at a host directory.
#[derive(Clone, Debug)]
pub struct ReadOnlyFs(LocalFs);

/// A virtual filesystem made from named, rooted host directories.
///
/// `Workspace` is intended for hosts like IDEs and agent harnesses that need to
/// expose several project-related roots through one guest-visible filesystem.
/// Guests see each mount as a top-level directory, for example `project/src` or
/// `scratch/notes.txt`, while the host keeps control over which mounts are
/// writable.
#[derive(Clone, Debug, Default)]
pub struct Workspace {
    mounts: Arc<Vec<WorkspaceMount>>,
}

/// Metadata for one workspace mount.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceMount {
    name: String,
    root: Arc<Root>,
    writable: bool,
}

impl ReadOnlyFs {
    /// Set a display-only logical root name, not a physical host path.
    pub fn with_logical_name(mut self, name: impl Into<String>) -> Self {
        self.0 = self.0.with_logical_name(name);
        self
    }

    pub fn new(root: impl Into<PathBuf>) -> Result<Self, FsError> {
        Ok(Self(LocalFs::with_permissions(root, false)?))
    }

    pub fn root(&self) -> &Path {
        self.0.root()
    }

    pub fn read_text(&self, path: impl AsRef<str>) -> Result<String, FsError> {
        self.0.read_text(path)
    }

    pub fn write_text(
        &self,
        path: impl AsRef<str>,
        contents: impl AsRef<str>,
    ) -> Result<(), FsError> {
        self.0.write_text(path, contents)
    }

    pub fn read_text_revision(&self, path: impl AsRef<str>) -> Result<RevisionedText, FsError> {
        self.0.read_text_revision(path)
    }

    pub fn read_text_revision_bounded(
        &self,
        path: impl AsRef<str>,
        max_bytes: usize,
    ) -> Result<RevisionedText, FsError> {
        self.0.read_text_revision_bounded(path, max_bytes)
    }

    /// Deny creation without touching the filesystem.
    pub fn create_text(
        &self,
        path: impl AsRef<str>,
        contents: impl AsRef<str>,
    ) -> Result<String, FsError> {
        self.0.create_text(path, contents)
    }

    pub fn save_text(
        &self,
        path: impl AsRef<str>,
        contents: impl AsRef<str>,
        expected_revision: Option<&str>,
    ) -> Result<SaveOutcome, FsError> {
        self.0.save_text(path, contents, expected_revision)
    }

    pub fn exists(&self, path: impl AsRef<str>) -> bool {
        self.0.exists(path)
    }

    /// Inspect an entry without following its final symlink.
    pub fn symlink_metadata(&self, path: impl AsRef<str>) -> Result<Metadata, FsError> {
        self.0.symlink_metadata(path)
    }

    pub fn list_dir(&self, path: impl AsRef<str>) -> Result<Vec<DirectoryEntry>, FsError> {
        self.0.list_dir(path)
    }

    pub fn create_dir(&self, path: impl AsRef<str>) -> Result<(), FsError> {
        self.0.create_dir(path)
    }

    pub fn remove_file(&self, path: impl AsRef<str>) -> Result<(), FsError> {
        self.0.remove_file(path)
    }

    pub fn remove_dir(&self, path: impl AsRef<str>) -> Result<(), FsError> {
        self.0.remove_dir(path)
    }
}

impl WorkspaceMount {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn root(&self) -> &Path {
        &self.root.path
    }

    pub fn is_writable(&self) -> bool {
        self.writable
    }
}

impl Workspace {
    /// Create an empty workspace.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the mounted roots in guest-visible order.
    pub fn mounts(&self) -> &[WorkspaceMount] {
        &self.mounts
    }

    /// Add a readable and writable local directory mount.
    pub fn mount(self, name: impl Into<String>, root: impl Into<PathBuf>) -> Result<Self, FsError> {
        self.with_mount(name, root, true)
    }

    /// Add a read-only local directory mount.
    pub fn mount_read_only(
        self,
        name: impl Into<String>,
        root: impl Into<PathBuf>,
    ) -> Result<Self, FsError> {
        self.with_mount(name, root, false)
    }

    fn with_mount(
        self,
        name: impl Into<String>,
        root: impl Into<PathBuf>,
        writable: bool,
    ) -> Result<Self, FsError> {
        let name = name.into();
        validate_mount_name(&name)?;

        if self.mounts.iter().any(|mount| mount.name == name) {
            return Err(FsError::new(FsErrorKind::Io, &name, "mount"));
        }

        let root = Root::open(root).map_err(|e| e.context(&name, "mount"))?;

        let mut mounts = (*self.mounts).clone();
        mounts.push(WorkspaceMount {
            name,
            root: Arc::new(root),
            writable,
        });
        mounts.sort_by(|left, right| left.name.cmp(&right.name));

        Ok(Self {
            mounts: Arc::new(mounts),
        })
    }

    pub fn read_text(&self, path: impl AsRef<str>) -> Result<String, FsError> {
        let path = path.as_ref();
        (|| {
            let (mount, relative_path) = self.resolve_mount(path.as_ref())?;
            read_text_at(&mount.root, &relative_path)
        })()
        .map_err(|e: FsError| e.context(path, "read-text"))
    }

    pub fn write_text(
        &self,
        path: impl AsRef<str>,
        contents: impl AsRef<str>,
    ) -> Result<(), FsError> {
        let path = path.as_ref();
        (|| {
            let (mount, relative_path) = self.resolve_mount(path.as_ref())?;
            if !mount.writable {
                return Err(FsError::new(
                    FsErrorKind::PermissionDenied,
                    path.as_ref(),
                    "write-text",
                ));
            }

            write_text_at(&mount.root, &relative_path, contents.as_ref())
        })()
        .map_err(|e: FsError| e.context(path, "write-text"))
    }

    pub fn read_text_revision(&self, path: impl AsRef<str>) -> Result<RevisionedText, FsError> {
        let contents = self.read_text(path)?;
        Ok(RevisionedText {
            revision: revision(contents.as_bytes()),
            contents,
        })
    }

    /// Create within a writable mount; see `LocalFs::create_text` for guarantees
    /// and the hostile temporary-namespace limitation.
    pub fn create_text(
        &self,
        path: impl AsRef<str>,
        contents: impl AsRef<str>,
    ) -> Result<String, FsError> {
        let path = path.as_ref();
        (|| {
            let (mount, relative) = self.resolve_mount(path)?;
            if !mount.writable {
                return Err(FsError::new(
                    FsErrorKind::PermissionDenied,
                    path,
                    "create-text",
                ));
            }
            create_text_at(&mount.root, &relative, contents.as_ref())
        })()
        .map_err(|e: FsError| e.context(path, "create-text"))
    }

    pub fn read_text_revision_bounded(
        &self,
        path: impl AsRef<str>,
        max_bytes: usize,
    ) -> Result<RevisionedText, FsError> {
        let path = path.as_ref();
        (|| {
            let (mount, relative) = self.resolve_mount(path)?;
            read_text_revision_bounded_at(&mount.root, &relative, max_bytes)
        })()
        .map_err(|e: FsError| e.context(path, "read-text-revision-bounded"))
    }

    pub fn save_text(
        &self,
        path: impl AsRef<str>,
        contents: impl AsRef<str>,
        expected_revision: Option<&str>,
    ) -> Result<SaveOutcome, FsError> {
        let path = path.as_ref();
        (|| {
            let (mount, relative) = self.resolve_mount(path)?;
            if !mount.writable {
                return Err(FsError::new(
                    FsErrorKind::PermissionDenied,
                    path,
                    "save-text",
                ));
            }
            save_text_at(&mount.root, &relative, contents.as_ref(), expected_revision)
        })()
        .map_err(|e: FsError| e.context(path, "save-text"))
    }

    pub fn exists(&self, path: impl AsRef<str>) -> bool {
        match self.resolve_workspace_path(path.as_ref()) {
            Ok(WorkspacePath::Root) => true,
            Ok(WorkspacePath::MountRoot(mount)) => {
                self.mounts.iter().any(|candidate| candidate.name == mount)
            }
            Ok(WorkspacePath::MountPath { mount, path }) => self
                .find_mount(&mount)
                .map(|mount| exists_at(&mount.root, &path))
                .unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Inspect an entry without following its final symlink.
    pub fn symlink_metadata(&self, path: impl AsRef<str>) -> Result<Metadata, FsError> {
        let path = path.as_ref();
        (|| match self.resolve_workspace_path(path.as_ref())? {
            WorkspacePath::Root => Ok(Metadata {
                file_type: FileType::Directory,
            }),
            _ => {
                let (mount, relative) = self.resolve_mount(path.as_ref())?;
                symlink_metadata_at(&mount.root, &relative)
            }
        })()
        .map_err(|e: FsError| e.context(path, "symlink-metadata"))
    }

    pub fn list_dir(&self, path: impl AsRef<str>) -> Result<Vec<DirectoryEntry>, FsError> {
        let path = path.as_ref();
        (|| match self.resolve_workspace_path(path.as_ref())? {
            WorkspacePath::Root => Ok(self
                .mounts
                .iter()
                .map(|mount| DirectoryEntry {
                    name: mount.name.clone(),
                    file_type: FileType::Directory,
                })
                .collect()),
            WorkspacePath::MountRoot(mount) => {
                let mount = self.mount_or_error(&mount)?;
                list_dir_at(&mount.root, ".")
            }
            WorkspacePath::MountPath { mount, path } => {
                let mount = self.mount_or_error(&mount)?;
                list_dir_at(&mount.root, &path)
            }
        })()
        .map_err(|e: FsError| e.context(path, "list-dir"))
    }

    pub fn create_dir(&self, path: impl AsRef<str>) -> Result<(), FsError> {
        let path = path.as_ref();
        (|| {
            let (mount, relative_path) = self.resolve_mount(path.as_ref())?;
            if !mount.writable {
                return Err(FsError::new(
                    FsErrorKind::PermissionDenied,
                    path.as_ref(),
                    "create-dir",
                ));
            }

            create_dir_at(&mount.root, &relative_path)
        })()
        .map_err(|e: FsError| e.context(path, "create-dir"))
    }

    pub fn remove_file(&self, path: impl AsRef<str>) -> Result<(), FsError> {
        let path = path.as_ref();
        (|| {
            let (mount, relative_path) = self.resolve_mount(path.as_ref())?;
            if !mount.writable {
                return Err(FsError::new(
                    FsErrorKind::PermissionDenied,
                    path.as_ref(),
                    "remove-file",
                ));
            }

            remove_file_at(&mount.root, &relative_path)
        })()
        .map_err(|e: FsError| e.context(path, "remove-file"))
    }

    pub fn remove_dir(&self, path: impl AsRef<str>) -> Result<(), FsError> {
        let path = path.as_ref();
        (|| {
            let (mount, relative_path) = self.resolve_mount(path.as_ref())?;
            if !mount.writable {
                return Err(FsError::new(
                    FsErrorKind::PermissionDenied,
                    path.as_ref(),
                    "remove-dir",
                ));
            }

            remove_dir_at(&mount.root, &relative_path)
        })()
        .map_err(|e: FsError| e.context(path, "remove-dir"))
    }

    fn resolve_mount(&self, path: &str) -> Result<(&WorkspaceMount, String), FsError> {
        match self.resolve_workspace_path(path)? {
            WorkspacePath::Root => Err(FsError::new(
                FsErrorKind::InvalidPath,
                path,
                "resolve-mount",
            )),
            WorkspacePath::MountRoot(mount) => Ok((self.mount_or_error(&mount)?, ".".to_string())),
            WorkspacePath::MountPath { mount, path } => Ok((self.mount_or_error(&mount)?, path)),
        }
    }

    fn resolve_workspace_path(&self, path: &str) -> Result<WorkspacePath, FsError> {
        let components = clean_workspace_path(path)?;
        if components.is_empty() {
            return Ok(WorkspacePath::Root);
        }

        let mount = components[0].clone();
        if components.len() == 1 {
            Ok(WorkspacePath::MountRoot(mount))
        } else {
            Ok(WorkspacePath::MountPath {
                mount,
                path: components[1..].join("/"),
            })
        }
    }

    fn find_mount(&self, name: &str) -> Option<&WorkspaceMount> {
        self.mounts.iter().find(|mount| mount.name == name)
    }

    fn mount_or_error(&self, name: &str) -> Result<&WorkspaceMount, FsError> {
        self.find_mount(name)
            .ok_or_else(|| FsError::new(FsErrorKind::NotFound, name, "resolve-mount"))
    }
}

impl bindings::cell::fs::fs::Host for LocalFs {
    fn read_text_revision(
        &mut self,
        path: String,
    ) -> Result<bindings::cell::fs::fs::RevisionedText, bindings::cell::fs::fs::FsError> {
        LocalFs::read_text_revision(self, path).map_err(Into::into)
    }

    fn save_text(
        &mut self,
        path: String,
        contents: String,
        expected_revision: Option<String>,
    ) -> Result<bindings::cell::fs::fs::SaveOutcome, bindings::cell::fs::fs::FsError> {
        LocalFs::save_text(self, path, contents, expected_revision.as_deref()).map_err(Into::into)
    }

    fn read_text(&mut self, path: String) -> Result<String, bindings::cell::fs::fs::FsError> {
        LocalFs::read_text(self, path).map_err(Into::into)
    }

    fn write_text(
        &mut self,
        path: String,
        contents: String,
    ) -> Result<(), bindings::cell::fs::fs::FsError> {
        LocalFs::write_text(self, path, contents).map_err(Into::into)
    }

    fn exists(&mut self, path: String) -> bool {
        LocalFs::exists(self, path)
    }

    fn symlink_metadata(
        &mut self,
        path: String,
    ) -> Result<Metadata, bindings::cell::fs::fs::FsError> {
        LocalFs::symlink_metadata(self, path).map_err(Into::into)
    }

    fn list_dir(
        &mut self,
        path: String,
    ) -> Result<Vec<DirectoryEntry>, bindings::cell::fs::fs::FsError> {
        LocalFs::list_dir(self, path).map_err(Into::into)
    }

    fn create_dir(&mut self, path: String) -> Result<(), bindings::cell::fs::fs::FsError> {
        LocalFs::create_dir(self, path).map_err(Into::into)
    }

    fn remove_file(&mut self, path: String) -> Result<(), bindings::cell::fs::fs::FsError> {
        LocalFs::remove_file(self, path).map_err(Into::into)
    }

    fn remove_dir(&mut self, path: String) -> Result<(), bindings::cell::fs::fs::FsError> {
        LocalFs::remove_dir(self, path).map_err(Into::into)
    }
}

impl bindings::cell::fs::fs::Host for ReadOnlyFs {
    fn read_text_revision(
        &mut self,
        path: String,
    ) -> Result<bindings::cell::fs::fs::RevisionedText, bindings::cell::fs::fs::FsError> {
        ReadOnlyFs::read_text_revision(self, path).map_err(Into::into)
    }

    fn save_text(
        &mut self,
        path: String,
        contents: String,
        expected_revision: Option<String>,
    ) -> Result<bindings::cell::fs::fs::SaveOutcome, bindings::cell::fs::fs::FsError> {
        ReadOnlyFs::save_text(self, path, contents, expected_revision.as_deref())
            .map_err(Into::into)
    }

    fn read_text(&mut self, path: String) -> Result<String, bindings::cell::fs::fs::FsError> {
        ReadOnlyFs::read_text(self, path).map_err(Into::into)
    }

    fn write_text(
        &mut self,
        path: String,
        contents: String,
    ) -> Result<(), bindings::cell::fs::fs::FsError> {
        ReadOnlyFs::write_text(self, path, contents).map_err(Into::into)
    }

    fn exists(&mut self, path: String) -> bool {
        ReadOnlyFs::exists(self, path)
    }

    fn symlink_metadata(
        &mut self,
        path: String,
    ) -> Result<Metadata, bindings::cell::fs::fs::FsError> {
        ReadOnlyFs::symlink_metadata(self, path).map_err(Into::into)
    }

    fn list_dir(
        &mut self,
        path: String,
    ) -> Result<Vec<DirectoryEntry>, bindings::cell::fs::fs::FsError> {
        ReadOnlyFs::list_dir(self, path).map_err(Into::into)
    }

    fn create_dir(&mut self, path: String) -> Result<(), bindings::cell::fs::fs::FsError> {
        ReadOnlyFs::create_dir(self, path).map_err(Into::into)
    }

    fn remove_file(&mut self, path: String) -> Result<(), bindings::cell::fs::fs::FsError> {
        ReadOnlyFs::remove_file(self, path).map_err(Into::into)
    }

    fn remove_dir(&mut self, path: String) -> Result<(), bindings::cell::fs::fs::FsError> {
        ReadOnlyFs::remove_dir(self, path).map_err(Into::into)
    }
}

impl bindings::cell::fs::fs::Host for Workspace {
    fn read_text_revision(
        &mut self,
        path: String,
    ) -> Result<bindings::cell::fs::fs::RevisionedText, bindings::cell::fs::fs::FsError> {
        Workspace::read_text_revision(self, path).map_err(Into::into)
    }

    fn save_text(
        &mut self,
        path: String,
        contents: String,
        expected_revision: Option<String>,
    ) -> Result<bindings::cell::fs::fs::SaveOutcome, bindings::cell::fs::fs::FsError> {
        Workspace::save_text(self, path, contents, expected_revision.as_deref()).map_err(Into::into)
    }

    fn read_text(&mut self, path: String) -> Result<String, bindings::cell::fs::fs::FsError> {
        Workspace::read_text(self, path).map_err(Into::into)
    }

    fn write_text(
        &mut self,
        path: String,
        contents: String,
    ) -> Result<(), bindings::cell::fs::fs::FsError> {
        Workspace::write_text(self, path, contents).map_err(Into::into)
    }

    fn exists(&mut self, path: String) -> bool {
        Workspace::exists(self, path)
    }

    fn symlink_metadata(
        &mut self,
        path: String,
    ) -> Result<Metadata, bindings::cell::fs::fs::FsError> {
        Workspace::symlink_metadata(self, path).map_err(Into::into)
    }

    fn list_dir(
        &mut self,
        path: String,
    ) -> Result<Vec<DirectoryEntry>, bindings::cell::fs::fs::FsError> {
        Workspace::list_dir(self, path).map_err(Into::into)
    }

    fn create_dir(&mut self, path: String) -> Result<(), bindings::cell::fs::fs::FsError> {
        Workspace::create_dir(self, path).map_err(Into::into)
    }

    fn remove_file(&mut self, path: String) -> Result<(), bindings::cell::fs::fs::FsError> {
        Workspace::remove_file(self, path).map_err(Into::into)
    }

    fn remove_dir(&mut self, path: String) -> Result<(), bindings::cell::fs::fs::FsError> {
        Workspace::remove_dir(self, path).map_err(Into::into)
    }
}

impl<T> cell::Capability<T> for LocalFs
where
    T: Send + 'static,
{
    fn name(&self) -> &'static str {
        "cell:fs/fs@0.1.0"
    }

    fn add_to_linker(self, linker: &mut Linker<T>) -> anyhow::Result<()> {
        add_fs_to_linker(linker, self.root, self.writable)
    }

    fn descriptor(&self) -> cell::CapabilityDescriptor {
        filesystem_descriptor(vec![root_descriptor(&self.logical_name, self.writable)])
    }
}

impl<T> cell::Capability<T> for ReadOnlyFs
where
    T: Send + 'static,
{
    fn name(&self) -> &'static str {
        "cell:fs/fs@0.1.0"
    }

    fn add_to_linker(self, linker: &mut Linker<T>) -> anyhow::Result<()> {
        add_fs_to_linker(linker, self.0.root, false)
    }

    fn descriptor(&self) -> cell::CapabilityDescriptor {
        filesystem_descriptor(vec![root_descriptor(&self.0.logical_name, false)])
    }
}

impl<T> cell::Capability<T> for Workspace
where
    T: Send + 'static,
{
    fn name(&self) -> &'static str {
        "cell:fs/fs@0.1.0"
    }

    fn add_to_linker(self, linker: &mut Linker<T>) -> anyhow::Result<()> {
        add_workspace_to_linker(linker, self)
    }

    fn descriptor(&self) -> cell::CapabilityDescriptor {
        filesystem_descriptor(
            self.mounts
                .iter()
                .map(|mount| root_descriptor(&mount.name, mount.writable))
                .collect(),
        )
    }
}

fn filesystem_descriptor(roots: Vec<cell::FilesystemRootDescriptor>) -> cell::CapabilityDescriptor {
    cell::CapabilityDescriptor {
        interface: "cell:fs/fs@0.1.0",
        authority: cell::AuthorityDescription::Filesystem { roots },
    }
}

fn root_descriptor(name: &str, writable: bool) -> cell::FilesystemRootDescriptor {
    use cell::{FilesystemOperation::*, FilesystemPolicy};
    let mut operations = vec![ReadText, ReadTextRevision, Exists, SymlinkMetadata, ListDir];
    if writable {
        operations.extend([WriteText, SaveText, CreateDir, RemoveFile, RemoveDir]);
    }
    cell::FilesystemRootDescriptor {
        name: name.to_owned(),
        read_policy: FilesystemPolicy::WithinRoot,
        write_policy: if writable {
            FilesystemPolicy::WithinRoot
        } else {
            FilesystemPolicy::Denied
        },
        operations,
    }
}

fn add_fs_to_linker<T>(
    linker: &mut Linker<T>,
    root: Arc<Root>,
    writable: bool,
) -> anyhow::Result<()>
where
    T: Send + 'static,
{
    let revision_fs = LocalFs {
        root: Arc::clone(&root),
        writable,
        logical_name: "/".to_owned(),
    };
    let save_fs = revision_fs.clone();
    let read_root = Arc::clone(&root);
    let write_root = Arc::clone(&root);
    let exists_root = Arc::clone(&root);
    let list_root = Arc::clone(&root);
    let metadata_root = Arc::clone(&root);
    let create_dir_root = Arc::clone(&root);
    let remove_file_root = Arc::clone(&root);
    let remove_dir_root = Arc::clone(&root);

    let mut instance = linker.instance("cell:fs/fs@0.1.0")?;

    instance.func_wrap("read-text-revision", move |_store, (path,): (String,)| {
        Ok((revision_fs
            .read_text_revision(path)
            .map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    instance.func_wrap(
        "save-text",
        move |_store, (path, contents, expected_revision): (String, String, Option<String>)| {
            Ok((save_fs
                .save_text(path, contents, expected_revision.as_deref())
                .map_err(bindings::cell::fs::fs::FsError::from),))
        },
    )?;

    instance.func_wrap("read-text", move |_store, (path,): (String,)| {
        Ok((read_text_at(&read_root, &path).map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    instance.func_wrap(
        "write-text",
        move |_store, (path, contents): (String, String)| {
            let result = if writable {
                write_text_at(&write_root, &path, &contents)
            } else {
                Err(FsError::new(
                    FsErrorKind::PermissionDenied,
                    &path,
                    "write-text",
                ))
            };

            Ok((result.map_err(bindings::cell::fs::fs::FsError::from),))
        },
    )?;

    instance.func_wrap("exists", move |_store, (path,): (String,)| {
        Ok((exists_at(&exists_root, &path),))
    })?;

    instance.func_wrap("symlink-metadata", move |_store, (path,): (String,)| {
        Ok((symlink_metadata_at(&metadata_root, &path)
            .map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    instance.func_wrap("list-dir", move |_store, (path,): (String,)| {
        Ok((list_dir_at(&list_root, &path).map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    instance.func_wrap("create-dir", move |_store, (path,): (String,)| {
        let result = if writable {
            create_dir_at(&create_dir_root, &path)
        } else {
            Err(FsError::new(
                FsErrorKind::PermissionDenied,
                &path,
                "create-dir",
            ))
        };

        Ok((result.map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    instance.func_wrap("remove-file", move |_store, (path,): (String,)| {
        let result = if writable {
            remove_file_at(&remove_file_root, &path)
        } else {
            Err(FsError::new(
                FsErrorKind::PermissionDenied,
                &path,
                "remove-file",
            ))
        };

        Ok((result.map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    instance.func_wrap("remove-dir", move |_store, (path,): (String,)| {
        let result = if writable {
            remove_dir_at(&remove_dir_root, &path)
        } else {
            Err(FsError::new(
                FsErrorKind::PermissionDenied,
                &path,
                "remove-dir",
            ))
        };

        Ok((result.map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    Ok(())
}

fn add_workspace_to_linker<T>(linker: &mut Linker<T>, workspace: Workspace) -> anyhow::Result<()>
where
    T: Send + 'static,
{
    let revision_workspace = workspace.clone();
    let save_workspace = workspace.clone();
    let read_workspace = workspace.clone();
    let write_workspace = workspace.clone();
    let exists_workspace = workspace.clone();
    let list_workspace = workspace.clone();
    let metadata_workspace = workspace.clone();
    let create_dir_workspace = workspace.clone();
    let remove_file_workspace = workspace.clone();
    let remove_dir_workspace = workspace;

    let mut instance = linker.instance("cell:fs/fs@0.1.0")?;

    instance.func_wrap("read-text-revision", move |_store, (path,): (String,)| {
        Ok((revision_workspace
            .read_text_revision(path)
            .map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    instance.func_wrap(
        "save-text",
        move |_store, (path, contents, expected_revision): (String, String, Option<String>)| {
            Ok((save_workspace
                .save_text(path, contents, expected_revision.as_deref())
                .map_err(bindings::cell::fs::fs::FsError::from),))
        },
    )?;

    instance.func_wrap("read-text", move |_store, (path,): (String,)| {
        Ok((read_workspace
            .read_text(path)
            .map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    instance.func_wrap(
        "write-text",
        move |_store, (path, contents): (String, String)| {
            Ok((write_workspace
                .write_text(path, contents)
                .map_err(bindings::cell::fs::fs::FsError::from),))
        },
    )?;

    instance.func_wrap("exists", move |_store, (path,): (String,)| {
        Ok((exists_workspace.exists(path),))
    })?;

    instance.func_wrap("symlink-metadata", move |_store, (path,): (String,)| {
        Ok((metadata_workspace
            .symlink_metadata(path)
            .map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    instance.func_wrap("list-dir", move |_store, (path,): (String,)| {
        Ok((list_workspace
            .list_dir(path)
            .map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    instance.func_wrap("create-dir", move |_store, (path,): (String,)| {
        Ok((create_dir_workspace
            .create_dir(path)
            .map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    instance.func_wrap("remove-file", move |_store, (path,): (String,)| {
        Ok((remove_file_workspace
            .remove_file(path)
            .map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    instance.func_wrap("remove-dir", move |_store, (path,): (String,)| {
        Ok((remove_dir_workspace
            .remove_dir(path)
            .map_err(bindings::cell::fs::fs::FsError::from),))
    })?;

    Ok(())
}

// cap-primitives 4 exposes no public typed escape marker. Only its exact
// synthetic error is confinement; OS EACCES/EPERM remain permission errors.
fn io_error(error: std::io::Error, path: &str, operation: &str) -> FsError {
    use std::io::ErrorKind;
    let kind = match error.kind() {
        ErrorKind::PermissionDenied
            if error.raw_os_error().is_none()
                && error.to_string() == "a path led outside of the filesystem" =>
        {
            FsErrorKind::ConfinementViolation
        }
        ErrorKind::PermissionDenied => FsErrorKind::PermissionDenied,
        ErrorKind::NotFound => FsErrorKind::NotFound,
        ErrorKind::InvalidInput => FsErrorKind::InvalidPath,
        _ => FsErrorKind::Io,
    };
    FsError::new(kind, path, operation)
}

fn read_text_at(root: &Root, path: &str) -> Result<String, FsError> {
    root.dir
        .read(clean_relative_path(path).map_err(|e| e.context(path, "read-text"))?)
        .map_err(|error| io_error(error, path, "read-text"))
        .and_then(|bytes| {
            String::from_utf8(bytes)
                .map_err(|_| FsError::new(FsErrorKind::InvalidTextEncoding, path, "read-text"))
        })
}

fn read_text_revision_bounded_at(
    root: &Root,
    path: &str,
    max_bytes: usize,
) -> Result<RevisionedText, FsError> {
    const OP: &str = "read-text-revision-bounded";
    let relative = clean_relative_path(path).map_err(|e| e.context(path, OP))?;
    #[cfg(not(unix))]
    {
        let _ = (root, relative, max_bytes);
        Err(FsError::new(FsErrorKind::UnsupportedOperation, path, OP))
    }
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        let mut options = cap_std::fs::OpenOptions::new();
        // Avoid blocking on a FIFO substituted between lookup and open.
        options.read(true).custom_flags(libc::O_NONBLOCK);
        let mut file = root
            .dir
            .open_with(relative, &options)
            .map_err(|e| io_error(e, path, OP))?;
        if !file
            .metadata()
            .map_err(|e| io_error(e, path, OP))?
            .is_file()
        {
            return Err(FsError::new(FsErrorKind::NonRegularFile, path, OP));
        }
        let bytes =
            read_bounded(&mut file, max_bytes).map_err(|kind| FsError::new(kind, path, OP))?;
        let token = revision(&bytes);
        let contents = String::from_utf8(bytes)
            .map_err(|_| FsError::new(FsErrorKind::InvalidTextEncoding, path, OP))?;
        Ok(RevisionedText {
            contents,
            revision: token,
        })
    }
}

#[cfg(test)]
mod bounded_reader_tests {
    use super::*;
    use std::io::{self, Read};

    #[test]
    fn growing_source_stops_after_one_extra_byte() {
        struct Growing(usize);
        impl Read for Growing {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                // Deterministically model content that keeps growing: no EOF.
                buffer.fill(b'x');
                self.0 += buffer.len();
                Ok(buffer.len())
            }
        }
        for limit in [0, 1, 8192, 8193, 32768] {
            let mut source = Growing(0);
            assert_eq!(
                read_bounded(&mut source, limit),
                Err(FsErrorKind::ContentTooLarge)
            );
            assert_eq!(source.0, limit + 1);
        }
    }

    #[test]
    fn maximum_limit_does_not_overflow_or_preallocate() {
        assert_eq!(
            read_bounded(&mut io::empty(), usize::MAX).unwrap(),
            Vec::<u8>::new()
        );
        struct Failing;
        impl Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::PermissionDenied.into())
            }
        }
        assert_eq!(
            read_bounded(&mut Failing, 8),
            Err(FsErrorKind::PermissionDenied)
        );
    }
}

// Fixed-size reads and exact reservation avoid metadata-sized allocation and
// read_to_end's speculative growth. Probe one extra byte without storing it.
fn read_bounded(reader: &mut impl std::io::Read, limit: usize) -> Result<Vec<u8>, FsErrorKind> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let remaining = limit - bytes.len();
        let size = remaining.min(buffer.len()).max(1);
        let n = match reader.read(&mut buffer[..size]) {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_error(e, "", "").kind),
        };
        if n == 0 {
            return Ok(bytes);
        }
        if n > remaining {
            return Err(FsErrorKind::ContentTooLarge);
        }
        bytes.try_reserve_exact(n).map_err(|_| FsErrorKind::Io)?;
        bytes.extend_from_slice(&buffer[..n]);
    }
}

fn write_text_at(root: &Root, path: &str, contents: &str) -> Result<(), FsError> {
    root.dir
        .write(
            clean_relative_path(path).map_err(|e| e.context(path, "write-text"))?,
            contents,
        )
        .map_err(|error| io_error(error, path, "write-text"))
}

fn revision(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn create_text_at(root: &Root, path: &str, contents: &str) -> Result<String, FsError> {
    let relative = clean_relative_path(path).map_err(|e| e.context(path, "create-text"))?;
    #[cfg(not(unix))]
    {
        let _ = (root, relative, contents);
        Err(FsError::new(
            FsErrorKind::UnsupportedOperation,
            path,
            "create-text",
        ))
    }
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        use std::io::{ErrorKind, Write};
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let run = || -> std::io::Result<String> {
            let name = relative
                .file_name()
                .ok_or_else(|| std::io::Error::from(ErrorKind::InvalidInput))?;
            let parent = root.dir.open_dir(
                relative
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?;
            let token = revision(contents.as_bytes());
            let mut options = cap_std::fs::OpenOptions::new();
            options.write(true).create_new(true).mode(0o600);
            let (temp, mut file) = loop {
                let temp = format!(
                    ".cell-create-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                );
                // Never use the destination itself as scratch, even if the
                // caller deliberately selected a generated temporary name.
                if name == std::ffi::OsStr::new(&temp) {
                    continue;
                }
                match parent.open_with(&temp, &options) {
                    Ok(file) => break (temp, file),
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e),
                }
            };
            let result = (|| {
                file.write_all(contents.as_bytes())?;
                drop(file);
                // cap-std 4 uses linkat with empty flags on Unix: the final
                // destination is neither followed nor replaced. The source
                // is still a name, not the retained file's identity.
                parent.hard_link(&temp, &parent, name)?;
                Ok(token)
            })();
            // Publication is the commit point. Cleanup failure must not turn
            // a successful create into a reported failure (or invite retries).
            let _ = parent.remove_file(&temp);
            result
        };
        run().map_err(|e| match e.kind() {
            ErrorKind::AlreadyExists => {
                FsError::new(FsErrorKind::AlreadyExists, path, "create-text")
            }
            ErrorKind::Unsupported => {
                FsError::new(FsErrorKind::UnsupportedOperation, path, "create-text")
            }
            _ => io_error(e, path, "create-text"),
        })
    }
}

fn save_text_at(
    root: &Root,
    path: &str,
    contents: &str,
    expected: Option<&str>,
) -> Result<SaveOutcome, FsError> {
    let relative = clean_relative_path(path).map_err(|e| e.context(path, "save-text"))?;
    #[cfg(not(unix))]
    {
        let _ = (root, relative, contents, expected);
        return Err(FsError::new(
            FsErrorKind::Other("unsupported-save-platform".into()),
            path,
            "save-text",
        ));
    }
    #[cfg(unix)]
    {
        use cap_std::fs::{OpenOptionsExt, PermissionsExt};
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let run = || -> std::io::Result<SaveOutcome> {
            let name = relative
                .file_name()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
            let parent = root.dir.open_dir(
                relative
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?;
            let metadata = parent.symlink_metadata(name)?;
            if !metadata.is_file() {
                return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
            }
            if metadata.permissions().readonly() {
                return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
            }
            if let Some(expected) = expected {
                let current = revision(&parent.read(name)?);
                if expected != current {
                    return Ok(SaveOutcome::Conflict(current));
                }
            }
            let mut options = cap_std::fs::OpenOptions::new();
            options.write(true).create_new(true).mode(0o600);
            let (temp, mut file) = loop {
                let temp = format!(
                    ".cell-save-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                );
                match parent.open_with(&temp, &options) {
                    Ok(file) => break (temp, file),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(e),
                }
            };
            let result = (|| {
                file.write_all(contents.as_bytes())?;
                file.set_permissions(cap_std::fs::Permissions::from_mode(
                    metadata.permissions().mode() & 0o777,
                ))?;
                // Close before rename; no fsync is promised by this operation.
                drop(file);
                parent.rename(&temp, &parent, name)?;
                Ok(SaveOutcome::Saved(revision(contents.as_bytes())))
            })();
            if result.is_err() {
                let _ = parent.remove_file(&temp);
            }
            result
        };
        run().map_err(|e| io_error(e, path, "save-text"))
    }
}

fn exists_at(root: &Root, path: &str) -> bool {
    clean_relative_path(path)
        .map(|path| root.dir.metadata(path).is_ok())
        .unwrap_or(false)
}

fn symlink_metadata_at(root: &Root, path: &str) -> Result<Metadata, FsError> {
    let relative =
        clean_relative_path_or_root(path).map_err(|e| e.context(path, "symlink-metadata"))?;
    let metadata = if relative.as_os_str().is_empty() {
        root.dir.dir_metadata()
    } else {
        root.dir.symlink_metadata(relative)
    }
    .map_err(|error| io_error(error, path, "symlink-metadata"))?;
    Ok(Metadata {
        file_type: classify_file_type(metadata.file_type()),
    })
}

fn list_dir_at(root: &Root, path: &str) -> Result<Vec<DirectoryEntry>, FsError> {
    let relative = clean_relative_path_or_root(path).map_err(|e| e.context(path, "list-dir"))?;
    let directory = if relative.as_os_str().is_empty() {
        root.dir.try_clone()
    } else {
        root.dir.open_dir(relative)
    }
    .map_err(|error| io_error(error, path, "list-dir"))?;
    let mut entries = directory
        .entries()
        .map_err(|error| io_error(error, path, "list-dir"))?
        .map(|entry| {
            let entry = entry.map_err(|error| io_error(error, path, "list-dir"))?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| FsError::new(FsErrorKind::InvalidTextEncoding, path, "list-dir"))?;
            let file_type = entry
                .file_type()
                .map_err(|error| io_error(error, path, "list-dir"))?;
            Ok(DirectoryEntry {
                name,
                file_type: classify_file_type(file_type),
            })
        })
        .collect::<Result<Vec<_>, FsError>>()?;
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

fn create_dir_at(root: &Root, path: &str) -> Result<(), FsError> {
    root.dir
        .create_dir(clean_relative_path(path).map_err(|e| e.context(path, "create-dir"))?)
        .map_err(|error| io_error(error, path, "create-dir"))
}

fn remove_file_at(root: &Root, path: &str) -> Result<(), FsError> {
    root.dir
        .remove_file(clean_relative_path(path).map_err(|e| e.context(path, "remove-file"))?)
        .map_err(|error| io_error(error, path, "remove-file"))
}

fn remove_dir_at(root: &Root, path: &str) -> Result<(), FsError> {
    root.dir
        .remove_dir(clean_relative_path(path).map_err(|e| e.context(path, "remove-dir"))?)
        .map_err(|error| io_error(error, path, "remove-dir"))
}

fn classify_file_type(file_type: cap_std::fs::FileType) -> FileType {
    if file_type.is_file() {
        FileType::File
    } else if file_type.is_dir() {
        FileType::Directory
    } else if file_type.is_symlink() {
        FileType::Symlink
    } else {
        FileType::Other
    }
}

enum WorkspacePath {
    Root,
    MountRoot(String),
    MountPath { mount: String, path: String },
}

fn validate_mount_name(name: &str) -> Result<(), FsError> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
    {
        return Err(FsError::new(FsErrorKind::InvalidPath, name, "mount"));
    }
    Ok(())
}

fn clean_workspace_path(guest_path: &str) -> Result<Vec<String>, FsError> {
    if guest_path.contains('\0') {
        return Err(FsError::new(
            FsErrorKind::InvalidPath,
            guest_path,
            "validate-path",
        ));
    }
    let path = Path::new(guest_path);
    let mut clean = Vec::new();

    for component in path.components() {
        match component {
            Component::Normal(segment) => clean.push(
                segment
                    .to_str()
                    .ok_or_else(|| {
                        FsError::new(FsErrorKind::InvalidPath, guest_path, "validate-path")
                    })?
                    .to_string(),
            ),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(FsError::new(
                    FsErrorKind::ConfinementViolation,
                    guest_path,
                    "validate-path",
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(FsError::new(
                    FsErrorKind::ConfinementViolation,
                    guest_path,
                    "validate-path",
                ));
            }
        }
    }

    Ok(clean)
}

fn clean_relative_path(guest_path: &str) -> Result<PathBuf, FsError> {
    if guest_path.contains('\0') {
        return Err(FsError::new(
            FsErrorKind::InvalidPath,
            guest_path,
            "validate-path",
        ));
    }
    let clean = clean_relative_path_or_root(guest_path)?;

    if clean.as_os_str().is_empty() {
        return Err(FsError::new(
            FsErrorKind::InvalidPath,
            guest_path,
            "validate-path",
        ));
    }

    Ok(clean)
}

fn clean_relative_path_or_root(guest_path: &str) -> Result<PathBuf, FsError> {
    if guest_path.contains('\0') {
        return Err(FsError::new(
            FsErrorKind::InvalidPath,
            guest_path,
            "validate-path",
        ));
    }
    let path = Path::new(guest_path);
    let mut clean = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Normal(segment) => clean.push(segment),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(FsError::new(
                    FsErrorKind::ConfinementViolation,
                    guest_path,
                    "validate-path",
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(FsError::new(
                    FsErrorKind::ConfinementViolation,
                    guest_path,
                    "validate-path",
                ));
            }
        }
    }

    Ok(clean)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cell::Capability;
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn descriptors_distinguish_authority_without_host_paths() -> anyhow::Result<()> {
        use cell::{AuthorityDescription, FilesystemOperation, FilesystemPolicy};
        let dir = TestDir::new()?;
        let read_only = ReadOnlyFs::new(dir.path())?.with_logical_name("project");
        assert!(read_only.write_text("denied.txt", "no").is_err());
        let ro = cell::Cell::new()?.with(read_only)?;
        let rw = cell::Cell::new()?.with(LocalFs::new(dir.path())?.with_logical_name("project"))?;
        assert_eq!(ro.capabilities(), rw.capabilities());
        assert_eq!(ro.capabilities(), &["cell:fs/fs@0.1.0"]);
        let roots = |cell: &cell::Cell| match &cell.capability_descriptors()[0].authority {
            AuthorityDescription::Filesystem { roots } => roots.clone(),
            _ => panic!("expected filesystem metadata"),
        };
        let ro_roots = roots(&ro);
        let rw_roots = roots(&rw);
        assert_eq!(ro_roots[0].name, "project");
        assert_eq!(ro_roots[0].read_policy, FilesystemPolicy::WithinRoot);
        assert_eq!(ro_roots[0].write_policy, FilesystemPolicy::Denied);
        assert_eq!(rw_roots[0].write_policy, FilesystemPolicy::WithinRoot);
        assert!(
            !ro_roots[0]
                .operations
                .contains(&FilesystemOperation::SaveText)
        );
        assert!(
            rw_roots[0]
                .operations
                .contains(&FilesystemOperation::SaveText)
        );
        assert!(
            !format!("{:?}", rw.capability_descriptors()).contains(dir.path().to_str().unwrap())
        );

        let workspace = Workspace::new()
            .mount_read_only("project", dir.path())?
            .mount("scratch", dir.path())?;
        let cell = cell::Cell::new()?.with(workspace)?;
        let mounts = roots(&cell);
        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0], ro_roots[0]);
        assert_eq!(mounts[1].name, "scratch");
        assert_eq!(mounts[1].write_policy, FilesystemPolicy::WithinRoot);
        let default = cell::Cell::new()?.with(LocalFs::new(dir.path())?)?;
        assert_eq!(roots(&default)[0].name, "/");
        Ok(())
    }

    #[test]
    fn permission_errors_are_not_broadly_classified_as_escapes() {
        use std::io::{Error, ErrorKind};
        for error in [
            Error::from_raw_os_error(13),
            Error::from_raw_os_error(1),
            Error::new(ErrorKind::PermissionDenied, "denied"),
        ] {
            assert_eq!(
                io_error(error, "file", "read-text").kind,
                FsErrorKind::PermissionDenied
            );
        }
        let escape = Error::new(
            ErrorKind::PermissionDenied,
            "a path led outside of the filesystem",
        );
        assert_eq!(
            io_error(escape, "file", "read-text").kind,
            FsErrorKind::ConfinementViolation
        );
    }

    #[test]
    fn local_fs_reads_text_beneath_root() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        fs::write(dir.path().join("note.txt"), "hello from fs")?;
        let fs = LocalFs::new(dir.path())?;

        assert_eq!(fs.read_text("note.txt").unwrap(), "hello from fs");
        Ok(())
    }

    #[test]
    fn local_fs_writes_text_beneath_root() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        let fs = LocalFs::new(dir.path())?;

        fs.write_text("note.txt", "written by guest").unwrap();

        assert_eq!(
            fs::read_to_string(dir.path().join("note.txt"))?,
            "written by guest"
        );
        Ok(())
    }

    #[test]
    fn read_only_fs_denies_writes() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        let fs = ReadOnlyFs::new(dir.path())?;

        assert_eq!(
            fs.write_text("note.txt", "nope").unwrap_err().kind,
            FsErrorKind::PermissionDenied
        );
        assert!(!dir.path().join("note.txt").exists());
        Ok(())
    }

    #[test]
    fn fs_rejects_paths_that_escape_root() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        let fs = LocalFs::new(dir.path())?;

        assert!(fs.read_text("../secret.txt").is_err());
        assert!(fs.write_text("../secret.txt", "secret").is_err());
        Ok(())
    }

    #[test]
    fn fs_rejects_symlinks_that_escape_root() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        let outside = TestDir::new()?;
        fs::write(outside.path().join("secret.txt"), "secret")?;

        #[cfg(unix)]
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            dir.path().join("link.txt"),
        )?;

        #[cfg(windows)]
        std::os::windows::fs::symlink_file(
            outside.path().join("secret.txt"),
            dir.path().join("link.txt"),
        )?;

        let fs = LocalFs::new(dir.path())?;

        assert!(fs.read_text("link.txt").is_err());
        Ok(())
    }

    #[test]
    fn exists_reports_only_accessible_existing_paths() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        fs::write(dir.path().join("note.txt"), "hello")?;
        let fs = LocalFs::new(dir.path())?;

        assert!(fs.exists("note.txt"));
        assert!(!fs.exists("missing.txt"));
        assert!(!fs.exists("../note.txt"));
        Ok(())
    }

    #[test]
    fn local_fs_lists_directories() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        fs::write(dir.path().join("note.txt"), "hello")?;
        fs::create_dir(dir.path().join("nested"))?;
        let fs = LocalFs::new(dir.path())?;

        let entries = fs.list_dir("").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "nested");
        assert!(matches!(entries[0].file_type, FileType::Directory));
        assert_eq!(entries[1].name, "note.txt");
        assert!(matches!(entries[1].file_type, FileType::File));
        assert_eq!(fs.list_dir(".").unwrap().len(), 2);
        assert!(fs.list_dir("../outside").is_err());
        Ok(())
    }

    #[test]
    fn local_fs_creates_and_removes_directories() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        let fs = LocalFs::new(dir.path())?;

        fs.create_dir("nested").unwrap();
        assert!(dir.path().join("nested").is_dir());

        fs.remove_dir("nested").unwrap();
        assert!(!dir.path().join("nested").exists());
        Ok(())
    }

    #[test]
    fn local_fs_removes_files() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        fs::write(dir.path().join("note.txt"), "hello")?;
        let fs = LocalFs::new(dir.path())?;

        fs.remove_file("note.txt").unwrap();
        assert!(!dir.path().join("note.txt").exists());
        Ok(())
    }

    #[test]
    fn read_only_fs_denies_directory_mutations() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        fs::write(dir.path().join("note.txt"), "hello")?;
        let fs = ReadOnlyFs::new(dir.path())?;

        assert_eq!(
            fs.create_dir("nested").unwrap_err().kind,
            FsErrorKind::PermissionDenied
        );
        assert_eq!(
            fs.remove_file("note.txt").unwrap_err().kind,
            FsErrorKind::PermissionDenied
        );
        assert_eq!(
            fs.remove_dir(".").unwrap_err().kind,
            FsErrorKind::PermissionDenied
        );
        assert!(dir.path().join("note.txt").exists());
        Ok(())
    }

    #[test]
    fn directory_mutations_reject_paths_that_escape_root() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        let fs = LocalFs::new(dir.path())?;

        assert!(fs.create_dir("../nested").is_err());
        assert!(fs.remove_file("../note.txt").is_err());
        assert!(fs.remove_dir("../nested").is_err());
        Ok(())
    }

    #[test]
    fn adds_fs_functions_to_cell_linker() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        let mut cell = cell::Cell::<()>::empty()?;

        LocalFs::new(dir.path())?.add_to_linker(cell.linker_mut())?;

        Ok(())
    }

    #[test]
    fn adding_fs_functions_twice_returns_an_error() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        let mut cell = cell::Cell::<()>::empty()?;

        LocalFs::new(dir.path())?.add_to_linker(cell.linker_mut())?;
        let duplicate_registration = LocalFs::new(dir.path())?.add_to_linker(cell.linker_mut());

        assert!(duplicate_registration.is_err());
        Ok(())
    }

    #[test]
    fn workspace_lists_mounts_as_virtual_root() -> anyhow::Result<()> {
        let project = TestDir::new()?;
        let scratch = TestDir::new()?;
        let workspace = Workspace::new()
            .mount_read_only("project", project.path())?
            .mount("scratch", scratch.path())?;

        let entries = workspace.list_dir(".").unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "project");
        assert!(matches!(entries[0].file_type, FileType::Directory));
        assert_eq!(entries[1].name, "scratch");
        assert!(workspace.exists("."));
        assert!(workspace.exists("project"));
        assert!(!workspace.exists("missing"));
        Ok(())
    }

    #[test]
    fn workspace_routes_reads_and_writes_to_mounts() -> anyhow::Result<()> {
        let project = TestDir::new()?;
        let scratch = TestDir::new()?;
        fs::write(project.path().join("README.md"), "project docs")?;
        let workspace = Workspace::new()
            .mount_read_only("project", project.path())?
            .mount("scratch", scratch.path())?;

        assert_eq!(
            workspace.read_text("project/README.md").unwrap(),
            "project docs"
        );

        workspace
            .write_text("scratch/notes.txt", "agent notes")
            .unwrap();

        assert_eq!(
            fs::read_to_string(scratch.path().join("notes.txt"))?,
            "agent notes"
        );
        Ok(())
    }

    #[test]
    fn workspace_enforces_mount_permissions_and_path_boundaries() -> anyhow::Result<()> {
        let project = TestDir::new()?;
        let scratch = TestDir::new()?;
        let workspace = Workspace::new()
            .mount_read_only("project", project.path())?
            .mount("scratch", scratch.path())?;

        assert_eq!(
            workspace
                .write_text("project/README.md", "nope")
                .unwrap_err()
                .kind,
            FsErrorKind::PermissionDenied
        );
        assert!(workspace.read_text("../secret.txt").is_err());
        assert!(
            workspace
                .write_text("scratch/../project/README.md", "nope")
                .is_err()
        );
        assert!(workspace.read_text("unknown/file.txt").is_err());
        Ok(())
    }

    #[test]
    fn workspace_lists_directories_inside_mounts() -> anyhow::Result<()> {
        let project = TestDir::new()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("README.md"), "project docs")?;
        let workspace = Workspace::new().mount_read_only("project", project.path())?;

        let entries = workspace.list_dir("project").unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "README.md");
        assert!(matches!(entries[0].file_type, FileType::File));
        assert_eq!(entries[1].name, "src");
        assert!(matches!(entries[1].file_type, FileType::Directory));
        Ok(())
    }

    #[test]
    fn workspace_adds_fs_functions_to_cell_linker() -> anyhow::Result<()> {
        let dir = TestDir::new()?;
        let mut cell = cell::Cell::<()>::empty()?;

        Workspace::new()
            .mount_read_only("project", dir.path())?
            .add_to_linker(cell.linker_mut())?;

        Ok(())
    }

    struct TestDir {
        path: PathBuf,
    }

    static NEXT_TEST_DIR_ID: AtomicUsize = AtomicUsize::new(0);

    impl TestDir {
        fn new() -> anyhow::Result<Self> {
            let id = NEXT_TEST_DIR_ID.fetch_add(1, Ordering::Relaxed);
            let name = format!("cell-capability-fs-test-{}-{id}", std::process::id());
            let path = std::env::temp_dir().join(name);
            fs::create_dir(&path)?;
            Ok(Self { path })
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
