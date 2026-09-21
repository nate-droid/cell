use cell_capability_fs::{FsError, FsErrorKind, LocalFs, ReadOnlyFs, Workspace};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "cell-fs-errors-{}-{}",
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

fn check(error: FsError, kind: FsErrorKind, path: &str, operation: &str, root: &Temp) {
    assert_eq!(error.kind, kind);
    assert_eq!(error.path, path);
    assert_eq!(error.operation, operation);
    assert!(!format!("{error:?} {error}").contains(root.0.to_str().unwrap()));
}

#[test]
fn every_operation_has_logical_context() {
    let root = Temp::new();
    let local = LocalFs::new(&root.0).unwrap();
    let workspace = Workspace::new().mount("grant", &root.0).unwrap();
    let readonly = ReadOnlyFs::new(&root.0).unwrap();
    for (op, error) in [
        ("read-text", local.read_text("missing/child").unwrap_err()),
        (
            "write-text",
            local.write_text("missing/child", "text").unwrap_err(),
        ),
        (
            "symlink-metadata",
            local.symlink_metadata("missing/child").unwrap_err(),
        ),
        ("list-dir", local.list_dir("missing/child").unwrap_err()),
        ("create-dir", local.create_dir("missing/child").unwrap_err()),
        (
            "remove-file",
            local.remove_file("missing/child").unwrap_err(),
        ),
        ("remove-dir", local.remove_dir("missing/child").unwrap_err()),
    ] {
        check(error, FsErrorKind::NotFound, "missing/child", op, &root);
    }
    for (op, error) in [
        (
            "read-text",
            workspace.read_text("grant/missing/child").unwrap_err(),
        ),
        (
            "write-text",
            workspace
                .write_text("grant/missing/child", "text")
                .unwrap_err(),
        ),
        (
            "symlink-metadata",
            workspace
                .symlink_metadata("grant/missing/child")
                .unwrap_err(),
        ),
        (
            "list-dir",
            workspace.list_dir("grant/missing/child").unwrap_err(),
        ),
        (
            "create-dir",
            workspace.create_dir("grant/missing/child").unwrap_err(),
        ),
        (
            "remove-file",
            workspace.remove_file("grant/missing/child").unwrap_err(),
        ),
        (
            "remove-dir",
            workspace.remove_dir("grant/missing/child").unwrap_err(),
        ),
    ] {
        check(
            error,
            FsErrorKind::NotFound,
            "grant/missing/child",
            op,
            &root,
        );
    }
    for (op, error) in [
        (
            "write-text",
            readonly.write_text("../child", "text").unwrap_err(),
        ),
        ("create-dir", readonly.create_dir("../child").unwrap_err()),
        ("remove-file", readonly.remove_file("../child").unwrap_err()),
        ("remove-dir", readonly.remove_dir("../child").unwrap_err()),
    ] {
        check(error, FsErrorKind::PermissionDenied, "../child", op, &root);
    }
    check(
        workspace.read_text("unknown/child").unwrap_err(),
        FsErrorKind::NotFound,
        "unknown/child",
        "read-text",
        &root,
    );
    assert!(!local.exists("missing"));
    assert!(!local.exists("../outside"));
    assert!(!local.exists(".")); // Preserve the existing local-root exists semantics.
    assert!(workspace.exists("."));
    assert!(workspace.exists("grant"));
}

#[test]
fn invalid_paths_encoding_and_io_are_distinct() {
    let root = Temp::new();
    let local = LocalFs::new(&root.0).unwrap();
    for path in ["", ".", "nul\0path"] {
        check(
            local.read_text(path).unwrap_err(),
            FsErrorKind::InvalidPath,
            path,
            "read-text",
            &root,
        );
    }
    for path in ["../outside", "/absolute"] {
        check(
            local.read_text(path).unwrap_err(),
            FsErrorKind::ConfinementViolation,
            path,
            "read-text",
            &root,
        );
    }
    fs::write(root.0.join("binary"), [0xff]).unwrap();
    check(
        local.read_text("binary").unwrap_err(),
        FsErrorKind::InvalidTextEncoding,
        "binary",
        "read-text",
        &root,
    );
    local.create_dir("existing").unwrap();
    check(
        local.create_dir("existing").unwrap_err(),
        FsErrorKind::Io,
        "existing",
        "create-dir",
        &root,
    );
    check(
        LocalFs::new(root.0.join("missing")).unwrap_err(),
        FsErrorKind::NotFound,
        ".",
        "open-root",
        &root,
    );
    check(
        Workspace::new()
            .mount("grant", root.0.join("missing"))
            .unwrap_err(),
        FsErrorKind::NotFound,
        "grant",
        "mount",
        &root,
    );
}

#[cfg(unix)]
#[test]
fn symlink_escape_has_precise_category() {
    use std::os::unix::fs::symlink;
    let root = Temp::new();
    let local = LocalFs::new(&root.0).unwrap();
    symlink("../outside", root.0.join("escape")).unwrap();
    check(
        local.read_text("escape").unwrap_err(),
        FsErrorKind::ConfinementViolation,
        "escape",
        "read-text",
        &root,
    );
    assert!(!local.exists("escape"));
}

// macOS filesystems reject these names before they can reach list_dir.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn non_utf8_names_report_invalid_text_encoding() {
    use std::os::unix::ffi::OsStringExt;
    let root = Temp::new();
    let local = LocalFs::new(&root.0).unwrap();
    fs::write(
        root.0.join(std::ffi::OsString::from_vec(vec![0xff])),
        "text",
    )
    .unwrap();
    check(
        local.list_dir(".").unwrap_err(),
        FsErrorKind::InvalidTextEncoding,
        ".",
        "list-dir",
        &root,
    );
}

#[cfg(unix)]
#[test]
fn real_os_permission_failure_is_not_confinement() {
    use std::os::unix::fs::PermissionsExt;
    let root = Temp::new();
    let path = root.0.join("private");
    fs::write(&path, "secret").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0)).unwrap();
    let result = LocalFs::new(&root.0).unwrap().read_text("private");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    // Privileged test users may bypass mode bits; classifier unit tests still run.
    if let Err(error) = result {
        check(
            error,
            FsErrorKind::PermissionDenied,
            "private",
            "read-text",
            &root,
        );
    }
}

#[test]
fn wit_contracts_are_synchronized() {
    let host = include_str!("../wit/fs.wit");
    assert_eq!(host, include_str!("../../guest/wit/deps/cell-fs/fs.wit"));
    assert_eq!(
        host,
        include_str!("../../../examples/fs-demo/wit/deps/cell-fs/fs.wit")
    );
}
