#![cfg(unix)]

use cell_capability_fs::{FsErrorKind, LocalFs, ReadOnlyFs, Workspace};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::{
        Arc, Barrier,
        atomic::{AtomicU64, Ordering},
    },
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cell-create-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn creates_complete_text_and_revision_without_scratch_leftovers() {
    let dir = Temp::new();
    fs::create_dir(dir.0.join("nested")).unwrap();
    let cap = LocalFs::new(&dir.0).unwrap();
    for (name, contents) in [("nested/text", "hello 🦀\n"), ("nested/empty", "")] {
        let token = cap.create_text(name, contents).unwrap();
        let read = cap.read_text_revision(name).unwrap();
        assert_eq!(read.contents, contents);
        assert_eq!(read.revision, token);
        assert_eq!(
            fs::metadata(dir.0.join(name)).unwrap().permissions().mode() & 0o077,
            0
        );
    }
    assert_eq!(fs::read_dir(dir.0.join("nested")).unwrap().count(), 2);
}

#[test]
fn existing_entries_are_never_replaced_or_followed() {
    let dir = Temp::new();
    fs::write(dir.0.join("file"), "original").unwrap();
    fs::create_dir(dir.0.join("directory")).unwrap();
    symlink("file", dir.0.join("link")).unwrap();
    symlink("missing", dir.0.join("dangling")).unwrap();
    let cap = LocalFs::new(&dir.0).unwrap();
    for name in ["file", "directory", "link", "dangling"] {
        let error = cap.create_text(name, "replacement").unwrap_err();
        assert_eq!(error.kind, FsErrorKind::AlreadyExists);
        assert_eq!(error.path, name);
        assert_eq!(error.operation, "create-text");
    }
    assert_eq!(fs::read_to_string(dir.0.join("file")).unwrap(), "original");
    assert_eq!(
        fs::read_link(dir.0.join("dangling")).unwrap(),
        PathBuf::from("missing")
    );
    assert!(!dir.0.join("missing").exists());
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 4);
}

#[test]
fn concurrent_creators_have_exactly_one_winner() {
    let dir = Temp::new();
    let cap = LocalFs::new(&dir.0).unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let cap = cap.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let contents = format!("{i}").repeat(128 * 1024);
                barrier.wait();
                (cap.create_text("winner", &contents), contents)
            })
        })
        .collect();
    let mut winners = 0;
    for thread in threads {
        let (result, contents) = thread.join().unwrap();
        match result {
            Ok(token) => {
                winners += 1;
                assert_eq!(cap.read_text("winner").unwrap(), contents);
                assert_eq!(cap.read_text_revision("winner").unwrap().revision, token);
            }
            Err(error) => assert_eq!(error.kind, FsErrorKind::AlreadyExists),
        }
    }
    assert_eq!(winners, 1);
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
}

#[test]
fn permissions_mount_routing_and_sanitized_errors() {
    let dir = Temp::new();
    let readonly = ReadOnlyFs::new(&dir.0).unwrap();
    assert_eq!(
        readonly.create_text("denied", "x").unwrap_err().kind,
        FsErrorKind::PermissionDenied
    );
    let workspace = Workspace::new()
        .mount("rw", &dir.0)
        .unwrap()
        .mount_read_only("ro", &dir.0)
        .unwrap();
    let token = workspace.create_text("rw/new", "text").unwrap();
    assert_eq!(
        workspace.read_text_revision("rw/new").unwrap().revision,
        token
    );
    let error = workspace.create_text("ro/denied", "x").unwrap_err();
    assert_eq!(error.kind, FsErrorKind::PermissionDenied);
    assert_eq!(error.path, "ro/denied");
    assert_eq!(error.operation, "create-text");
    let error = workspace.create_text("rw/missing/file", "x").unwrap_err();
    assert_eq!(error.kind, FsErrorKind::NotFound);
    assert_eq!(error.path, "rw/missing/file");
    assert_eq!(error.operation, "create-text");
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
}

#[test]
fn rejects_escape_paths_and_escaping_parent_symlinks() {
    let dir = Temp::new();
    let outside = Temp::new();
    symlink(&outside.0, dir.0.join("escape")).unwrap();
    let cap = LocalFs::new(&dir.0).unwrap();
    for path in ["../outside", "/absolute", "escape/file"] {
        assert!(cap.create_text(path, "x").is_err());
    }
    assert_eq!(fs::read_dir(&outside.0).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
}
