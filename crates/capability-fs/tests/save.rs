#![cfg(unix)]
use cell_capability_fs::{FsErrorKind, LocalFs, ReadOnlyFs, SaveOutcome, Workspace};
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cell-save-test-{}-{}",
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
fn replaces_and_returns_content_token() {
    let dir = Temp::new();
    fs::write(dir.0.join("note"), "old").unwrap();
    fs::set_permissions(dir.0.join("note"), fs::Permissions::from_mode(0o640)).unwrap();
    let cap = LocalFs::new(&dir.0).unwrap();
    let old = cap.read_text_revision("note").unwrap();
    let held = fs::File::open(dir.0.join("note")).unwrap();
    let SaveOutcome::Saved(token) = cap.save_text("note", "new", Some(&old.revision)).unwrap()
    else {
        panic!()
    };
    assert_eq!(cap.read_text("note").unwrap(), "new");
    assert_eq!(token, cap.read_text_revision("note").unwrap().revision);
    assert_eq!(
        fs::metadata(dir.0.join("note"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o640
    );
    use std::io::Read;
    let mut old_contents = String::new();
    (&held).read_to_string(&mut old_contents).unwrap();
    assert_eq!(old_contents, "old");
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
}
#[test]
fn stale_token_conflicts_but_unconditional_save_replaces() {
    let dir = Temp::new();
    fs::write(dir.0.join("note"), "old").unwrap();
    let cap = LocalFs::new(&dir.0).unwrap();
    let old = cap.read_text_revision("note").unwrap();
    fs::write(dir.0.join("note"), "external").unwrap();
    let SaveOutcome::Conflict(current) =
        cap.save_text("note", "mine", Some(&old.revision)).unwrap()
    else {
        panic!()
    };
    assert_eq!(current, cap.read_text_revision("note").unwrap().revision);
    assert_eq!(cap.read_text("note").unwrap(), "external");
    assert!(matches!(
        cap.save_text("note", "mine", None).unwrap(),
        SaveOutcome::Saved(_)
    ));
    assert_eq!(cap.read_text("note").unwrap(), "mine");
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
}
#[test]
fn permission_failures_do_not_modify_target() {
    let dir = Temp::new();
    fs::write(dir.0.join("note"), "old").unwrap();
    assert_eq!(
        ReadOnlyFs::new(&dir.0)
            .unwrap()
            .save_text("note", "new", None)
            .unwrap_err()
            .kind,
        FsErrorKind::PermissionDenied
    );
    let workspace = Workspace::new().mount_read_only("project", &dir.0).unwrap();
    assert_eq!(
        workspace
            .save_text("project/note", "new", None)
            .unwrap_err()
            .kind,
        FsErrorKind::PermissionDenied
    );
    fs::set_permissions(dir.0.join("note"), fs::Permissions::from_mode(0o444)).unwrap();
    assert_eq!(
        LocalFs::new(&dir.0)
            .unwrap()
            .save_text("note", "new", None)
            .unwrap_err()
            .kind,
        FsErrorKind::PermissionDenied
    );
    assert_eq!(fs::read_to_string(dir.0.join("note")).unwrap(), "old");
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
}
#[test]
fn confinement_and_symlink_rules() {
    let dir = Temp::new();
    let outside = Temp::new();
    fs::write(outside.0.join("note"), "outside").unwrap();
    fs::create_dir(dir.0.join("sub")).unwrap();
    fs::write(dir.0.join("sub/note"), "inside").unwrap();
    symlink("sub", dir.0.join("alias")).unwrap();
    symlink("sub/note", dir.0.join("final")).unwrap();
    symlink(outside.0.join("note"), dir.0.join("outside-final")).unwrap();
    symlink(&outside.0, dir.0.join("escape")).unwrap();
    let cap = LocalFs::new(&dir.0).unwrap();
    assert!(matches!(
        cap.save_text("alias/note", "new", None).unwrap(),
        SaveOutcome::Saved(_)
    ));
    for path in ["final", "outside-final"] {
        assert_eq!(
            cap.save_text(path, "bad", None).unwrap_err().kind,
            FsErrorKind::InvalidPath
        );
        assert!(fs::symlink_metadata(dir.0.join(path)).unwrap().is_symlink());
    }
    for path in ["../note", "/note", "escape/note"] {
        assert_eq!(
            cap.save_text(path, "bad", None).unwrap_err().kind,
            FsErrorKind::ConfinementViolation
        );
    }
    assert_eq!(
        fs::read_to_string(outside.0.join("note")).unwrap(),
        "outside"
    );
    assert_eq!(
        cap.save_text("missing", "new", None).unwrap_err().kind,
        FsErrorKind::NotFound
    );
}
