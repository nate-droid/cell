#![cfg(unix)]

use cell_capability_fs::{FileType, FsError, LocalFs, ReadOnlyFs, Workspace};
use std::{
    fs,
    os::unix::{fs::symlink, net::UnixListener},
    path::{Path, PathBuf},
    sync::{
        Barrier,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        Self::new_in(&std::env::temp_dir())
    }

    fn new_in(parent: &Path) -> Self {
        loop {
            let path = parent.join(format!(
                "cell-fs-confinement-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create test directory: {error}"),
            }
        }
    }

    fn join(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    Local,
    ReadOnly,
    Workspace,
}

const KINDS: [Kind; 3] = [Kind::Local, Kind::ReadOnly, Kind::Workspace];

// Keep the same confinement assertions for local roots and workspace mounts.
enum Capability {
    Local(LocalFs),
    ReadOnly(ReadOnlyFs),
    Workspace(Workspace),
}

macro_rules! dispatch {
    ($self:expr, $path:expr, $method:ident $(, $arg:expr)*) => {
        match $self {
            Capability::Local(fs) => fs.$method($path $(, $arg)*),
            Capability::ReadOnly(fs) => fs.$method($path $(, $arg)*),
            Capability::Workspace(fs) => fs.$method(format!("grant/{}", $path) $(, $arg)*),
        }
    };
}

impl Capability {
    fn new(kind: Kind, root: &Path) -> Self {
        match kind {
            Kind::Local => Self::Local(LocalFs::new(root).unwrap()),
            Kind::ReadOnly => Self::ReadOnly(ReadOnlyFs::new(root).unwrap()),
            Kind::Workspace => Self::Workspace(Workspace::new().mount("grant", root).unwrap()),
        }
    }

    fn read(&self, path: &str) -> Result<String, FsError> {
        dispatch!(self, path, read_text)
    }

    fn write(&self, path: &str) -> Result<(), FsError> {
        dispatch!(self, path, write_text, "changed")
    }

    fn mkdir(&self, path: &str) -> Result<(), FsError> {
        dispatch!(self, path, create_dir)
    }

    fn unlink(&self, path: &str) -> Result<(), FsError> {
        dispatch!(self, path, remove_file)
    }

    fn rmdir(&self, path: &str) -> Result<(), FsError> {
        dispatch!(self, path, remove_dir)
    }

    fn metadata_type(&self, path: &str) -> Result<FileType, FsError> {
        dispatch!(self, path, symlink_metadata).map(|metadata| metadata.file_type)
    }

    fn entries(&self, path: &str) -> Result<Vec<(String, FileType)>, FsError> {
        dispatch!(self, path, list_dir).map(|entries| {
            entries
                .into_iter()
                .map(|entry| (entry.name, entry.file_type))
                .collect()
        })
    }
}

fn denied<T: std::fmt::Debug>(result: Result<T, FsError>, temp: &TempDir) {
    let error = result.expect_err("operation must not escape its grant");
    assert!(
        !error.to_string().contains(temp.0.to_str().unwrap()),
        "host path leaked: {error}"
    );
    assert!(
        !error
            .to_string()
            .contains(temp.join("root").to_str().unwrap()),
        "root leaked: {error}"
    );
    assert!(
        !error
            .to_string()
            .contains(temp.join("outside").to_str().unwrap()),
        "target leaked: {error}"
    );
}

#[test]
fn final_outside_symlink_write_cannot_truncate_or_create_target() {
    for kind in KINDS {
        let temp = TempDir::new();
        fs::create_dir(temp.join("root")).unwrap();
        fs::create_dir(temp.join("outside")).unwrap();
        fs::write(temp.join("outside/existing"), "untouched").unwrap();
        for (name, target) in [("existing", "existing"), ("dangling", "missing")] {
            symlink(
                format!("../outside/{target}"),
                temp.join(&format!("root/{name}")),
            )
            .unwrap();
        }
        let capability = Capability::new(kind, &temp.join("root"));
        denied(capability.write("existing"), &temp);
        denied(capability.write("dangling"), &temp);
        assert_eq!(
            fs::read_to_string(temp.join("outside/existing")).unwrap(),
            "untouched"
        );
        assert!(!temp.join("outside/missing").exists());
    }
}

#[test]
fn intermediate_symlink_escape_is_rejected_by_every_operation() {
    for kind in KINDS {
        let temp = TempDir::new();
        fs::create_dir(temp.join("root")).unwrap();
        fs::create_dir(temp.join("outside")).unwrap();
        fs::create_dir(temp.join("outside/empty")).unwrap();
        fs::write(temp.join("outside/file"), "untouched").unwrap();
        symlink("../outside", temp.join("root/escape")).unwrap();
        let capability = Capability::new(kind, &temp.join("root"));
        denied(capability.read("escape/file"), &temp);
        denied(capability.write("escape/file"), &temp);
        denied(capability.write("escape/new"), &temp);
        denied(capability.entries("escape"), &temp);
        denied(capability.mkdir("escape/new-dir"), &temp);
        denied(capability.unlink("escape/file"), &temp);
        denied(capability.rmdir("escape/empty"), &temp);
        denied(capability.metadata_type("escape/file"), &temp);
        assert_eq!(
            fs::read_to_string(temp.join("outside/file")).unwrap(),
            "untouched"
        );
        assert!(temp.join("outside/empty").is_dir());
        assert!(!temp.join("outside/new").exists());
        assert!(!temp.join("outside/new-dir").exists());
    }
}

#[test]
fn unlink_final_symlink_removes_only_the_link() {
    for kind in [Kind::Local, Kind::Workspace] {
        let temp = TempDir::new();
        fs::create_dir(temp.join("root")).unwrap();
        fs::create_dir(temp.join("outside")).unwrap();
        fs::write(temp.join("outside/file"), "untouched").unwrap();
        symlink("../outside/file", temp.join("root/link")).unwrap();
        symlink("../outside/missing", temp.join("root/dangling")).unwrap();
        symlink("../outside", temp.join("root/directory-link")).unwrap();
        let capability = Capability::new(kind, &temp.join("root"));
        for name in ["link", "dangling", "directory-link"] {
            capability.unlink(name).unwrap();
            assert!(fs::symlink_metadata(temp.join(&format!("root/{name}"))).is_err());
        }
        assert_eq!(
            fs::read_to_string(temp.join("outside/file")).unwrap(),
            "untouched"
        );
        assert!(temp.join("outside").is_dir());
    }
}

#[test]
fn contained_relative_symlinks_are_followed() {
    for kind in KINDS {
        let temp = TempDir::new();
        fs::create_dir(temp.join("root")).unwrap();
        fs::create_dir(temp.join("root/data")).unwrap();
        fs::create_dir(temp.join("root/links")).unwrap();
        fs::write(temp.join("root/data/file"), "original").unwrap();
        symlink("../data", temp.join("root/links/data")).unwrap();
        symlink("data/file", temp.join("root/file-link")).unwrap();
        symlink("data/new", temp.join("root/new-link")).unwrap();
        let capability = Capability::new(kind, &temp.join("root"));
        assert_eq!(capability.read("links/data/file").unwrap(), "original");
        assert_eq!(capability.read("file-link").unwrap(), "original");
        assert!(
            capability
                .entries("links/data")
                .unwrap()
                .iter()
                .any(|(name, _)| name == "file")
        );
        if !matches!(kind, Kind::ReadOnly) {
            capability.write("file-link").unwrap();
            capability.write("new-link").unwrap();
            capability.mkdir("links/data/new-dir").unwrap();
            assert_eq!(
                fs::read_to_string(temp.join("root/data/file")).unwrap(),
                "changed"
            );
            assert_eq!(
                fs::read_to_string(temp.join("root/data/new")).unwrap(),
                "changed"
            );
            capability.unlink("links/data/new").unwrap();
            capability.rmdir("links/data/new-dir").unwrap();
        }
    }
}

#[test]
fn absolute_symlink_targets_are_rejected_even_inside_root() {
    for kind in KINDS {
        let temp = TempDir::new();
        fs::create_dir(temp.join("root")).unwrap();
        fs::create_dir(temp.join("root/data")).unwrap();
        fs::write(temp.join("root/data/file"), "untouched").unwrap();
        symlink(temp.join("root/data/file"), temp.join("root/absolute-file")).unwrap();
        symlink(temp.join("root/data"), temp.join("root/absolute-dir")).unwrap();
        let capability = Capability::new(kind, &temp.join("root"));
        denied(capability.read("absolute-file"), &temp);
        denied(capability.write("absolute-file"), &temp);
        denied(capability.entries("absolute-dir"), &temp);
        denied(capability.read("absolute-dir/file"), &temp);
        assert!(matches!(
            capability.metadata_type("absolute-file").unwrap(),
            FileType::Symlink
        ));
        assert_eq!(
            fs::read_to_string(temp.join("root/data/file")).unwrap(),
            "untouched"
        );
    }
}

#[test]
fn metadata_and_listing_do_not_follow_final_symlinks() {
    for kind in KINDS {
        let temp = TempDir::new();
        fs::create_dir(temp.join("root")).unwrap();
        fs::create_dir(temp.join("root/directory")).unwrap();
        fs::write(temp.join("root/file"), "text").unwrap();
        symlink("file", temp.join("root/link")).unwrap();
        symlink("missing", temp.join("root/dangling")).unwrap();
        symlink("cycle-b", temp.join("root/cycle-a")).unwrap();
        symlink("cycle-a", temp.join("root/cycle-b")).unwrap();
        symlink("../outside", temp.join("root/outside-link")).unwrap();
        let capability = Capability::new(kind, &temp.join("root"));
        let entries = capability.entries(".").unwrap();
        for (name, expected) in [
            ("file", FileType::File),
            ("directory", FileType::Directory),
            ("link", FileType::Symlink),
            ("dangling", FileType::Symlink),
            ("cycle-a", FileType::Symlink),
            ("cycle-b", FileType::Symlink),
            ("outside-link", FileType::Symlink),
        ] {
            assert_eq!(
                std::mem::discriminant(&capability.metadata_type(name).unwrap()),
                std::mem::discriminant(&expected)
            );
            assert!(entries.iter().any(|(entry, ty)| {
                entry == name && std::mem::discriminant(ty) == std::mem::discriminant(&expected)
            }));
        }
        denied(capability.read("cycle-a"), &temp);
        denied(capability.metadata_type("missing"), &temp);
    }
}

#[test]
fn replacing_root_path_does_not_retarget_opened_capabilities() {
    for kind in KINDS {
        let temp = TempDir::new();
        fs::create_dir(temp.join("root")).unwrap();
        fs::write(temp.join("root/file"), "opened directory").unwrap();
        fs::write(temp.join("root/remove-me"), "old").unwrap();
        fs::create_dir(temp.join("root/remove-dir")).unwrap();
        let capability = Capability::new(kind, &temp.join("root"));
        fs::rename(temp.join("root"), temp.join("original")).unwrap();
        fs::create_dir(temp.join("root")).unwrap();
        fs::write(temp.join("root/file"), "replacement directory").unwrap();
        assert_eq!(capability.read("file").unwrap(), "opened directory");
        assert!(matches!(
            capability.metadata_type("remove-me").unwrap(),
            FileType::File
        ));
        assert!(
            capability
                .entries(".")
                .unwrap()
                .iter()
                .any(|(name, _)| name == "remove-me")
        );
        if !matches!(kind, Kind::ReadOnly) {
            capability.write("file").unwrap();
            capability.mkdir("created").unwrap();
            capability.unlink("remove-me").unwrap();
            capability.rmdir("remove-dir").unwrap();
            assert_eq!(
                fs::read_to_string(temp.join("original/file")).unwrap(),
                "changed"
            );
            assert!(temp.join("original/created").is_dir());
            assert!(!temp.join("original/remove-me").exists());
            assert!(!temp.join("original/remove-dir").exists());
        }
        assert_eq!(
            fs::read_to_string(temp.join("root/file")).unwrap(),
            "replacement directory"
        );
        assert!(!temp.join("root/created").exists());
    }
}

#[test]
fn read_only_workspace_mount_rejects_mutations() {
    let temp = TempDir::new();
    fs::write(temp.join("file"), "untouched").unwrap();
    fs::create_dir(temp.join("directory")).unwrap();
    let workspace = Workspace::new().mount_read_only("grant", &temp.0).unwrap();
    assert_eq!(workspace.read_text("grant/file").unwrap(), "untouched");
    assert!(workspace.write_text("grant/file", "changed").is_err());
    assert!(workspace.create_dir("grant/new").is_err());
    assert!(workspace.remove_file("grant/file").is_err());
    assert!(workspace.remove_dir("grant/directory").is_err());
    assert!(matches!(
        workspace.symlink_metadata("grant/file").unwrap().file_type,
        FileType::File
    ));
    assert_eq!(fs::read_to_string(temp.join("file")).unwrap(), "untouched");
    assert!(temp.join("directory").is_dir());
}

const RACE_ITERATIONS: usize = 1_000;

#[test]
fn racing_final_symlink_replacement_cannot_write_outside() {
    for kind in [Kind::Local, Kind::Workspace] {
        let temp = TempDir::new();
        fs::create_dir(temp.join("root")).unwrap();
        fs::create_dir(temp.join("outside")).unwrap();
        fs::write(temp.join("outside/sentinel"), "untouched").unwrap();
        fs::write(temp.join("root/inside"), "inside").unwrap();
        symlink("inside", temp.join("root/output")).unwrap();
        let capability = Capability::new(kind, &temp.join("root"));
        let start = Barrier::new(2);
        // Scoped workers are joined even if the operation loop unwinds.
        thread::scope(|scope| {
            let worker = scope.spawn(|| -> std::io::Result<()> {
                start.wait();
                for _ in 0..RACE_ITERATIONS {
                    for target in ["../outside/sentinel", "inside"] {
                        symlink(target, temp.join("root/prepared"))?;
                        fs::rename(temp.join("root/prepared"), temp.join("root/output"))?;
                        thread::yield_now();
                    }
                }
                Ok(())
            });
            start.wait();
            for _ in 0..RACE_ITERATIONS {
                // Either confined success or a resolution error is valid during a swap.
                let _ = capability.write("output");
                thread::yield_now();
            }
            worker
                .join()
                .expect("swap worker panicked")
                .expect("swap failed");
        });
        assert_eq!(
            fs::read_to_string(temp.join("outside/sentinel")).unwrap(),
            "untouched"
        );
    }
}

#[test]
fn racing_intermediate_symlink_replacement_keeps_operations_confined() {
    for kind in KINDS {
        let temp = TempDir::new();
        fs::create_dir(temp.join("root")).unwrap();
        fs::create_dir(temp.join("root/inside")).unwrap();
        fs::create_dir(temp.join("root/inside/remove-dir")).unwrap();
        fs::create_dir(temp.join("outside")).unwrap();
        fs::write(temp.join("root/inside/sentinel"), "inside").unwrap();
        fs::write(temp.join("outside/sentinel"), "untouched").unwrap();
        fs::write(temp.join("outside/remove-me"), "untouched").unwrap();
        fs::create_dir(temp.join("outside/remove-dir")).unwrap();
        symlink("inside", temp.join("root/route")).unwrap();
        let capability = Capability::new(kind, &temp.join("root"));
        let start = Barrier::new(2);
        let mut escaped_read = None;
        thread::scope(|scope| {
            let worker = scope.spawn(|| -> std::io::Result<()> {
                start.wait();
                for _ in 0..RACE_ITERATIONS {
                    for target in ["../outside", "inside"] {
                        symlink(target, temp.join("root/prepared"))?;
                        fs::rename(temp.join("root/prepared"), temp.join("root/route"))?;
                        thread::yield_now();
                    }
                }
                Ok(())
            });
            start.wait();
            for _ in 0..RACE_ITERATIONS {
                if let Ok(contents) = capability.read("route/sentinel") {
                    if contents != "inside" && contents != "changed" {
                        escaped_read = Some(contents);
                    }
                }
                let _ = capability.write("route/sentinel");
                let _ = capability.mkdir("route/new-dir");
                let _ = capability.write("route/remove-me");
                let _ = capability.unlink("route/remove-me");
                let _ = capability.rmdir("route/remove-dir");

                thread::yield_now();
            }
            worker
                .join()
                .expect("swap worker panicked")
                .expect("swap failed");
        });
        assert_eq!(escaped_read, None, "read escaped the grant");
        assert_eq!(
            fs::read_to_string(temp.join("outside/sentinel")).unwrap(),
            "untouched"
        );
        assert_eq!(
            fs::read_to_string(temp.join("outside/remove-me")).unwrap(),
            "untouched"
        );
        assert!(temp.join("outside/remove-dir").is_dir());
        assert!(!temp.join("outside/new-dir").exists());
    }
}

#[test]
fn unix_socket_metadata_and_listing_report_other() {
    for kind in KINDS {
        // Keep the socket path short without depending on the system temp path.
        // Cargo runs integration tests in the package directory; use a relative bind path.
        let temp = TempDir::new_in(Path::new("."));
        let _listener = UnixListener::bind(temp.join("socket")).unwrap();
        let capability = Capability::new(kind, &temp.0);
        assert!(matches!(
            capability.metadata_type("socket").unwrap(),
            FileType::Other
        ));
        assert!(
            capability
                .entries(".")
                .unwrap()
                .iter()
                .any(|(name, ty)| { name == "socket" && matches!(ty, FileType::Other) })
        );
    }
}
