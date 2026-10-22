#![cfg(unix)]
use cell_capability_fs::{FsErrorKind, LocalFs, ReadOnlyFs, SaveOutcome, Workspace};
use std::{fs, os::unix::fs::symlink};

#[test]
fn bounded_revision_contract() {
    let root = std::env::temp_dir().join(format!("cell-bounded-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let cap = LocalFs::new(&root).unwrap();
    fs::write(root.join("empty"), "").unwrap();
    assert_eq!(
        cap.read_text_revision_bounded("empty", 0).unwrap().contents,
        ""
    );
    fs::write(root.join("note"), "abc").unwrap();
    let snapshot = cap.read_text_revision_bounded("note", 3).unwrap();
    assert_eq!(snapshot.contents, "abc");
    for limit in [0, 2] {
        assert_eq!(
            cap.read_text_revision_bounded("note", limit)
                .unwrap_err()
                .kind,
            FsErrorKind::ContentTooLarge
        );
    }
    assert!(matches!(
        cap.save_text("note", "new", Some(&snapshot.revision))
            .unwrap(),
        SaveOutcome::Saved(_)
    ));
    fs::write(root.join("bad"), [0xff]).unwrap();
    assert_eq!(
        cap.read_text_revision_bounded("bad", 1).unwrap_err().kind,
        FsErrorKind::InvalidTextEncoding
    );
    for path in ["missing", "missing/child"] {
        assert_eq!(
            cap.read_text_revision_bounded(path, 8).unwrap_err().kind,
            FsErrorKind::NotFound
        );
    }
    fs::create_dir(root.join("dir")).unwrap();
    assert_eq!(
        cap.read_text_revision_bounded("dir", 8).unwrap_err().kind,
        FsErrorKind::NonRegularFile
    );
    symlink("note", root.join("link")).unwrap();
    assert_eq!(
        cap.read_text_revision_bounded("link", 3).unwrap().contents,
        "new"
    );
    symlink("../outside", root.join("escape")).unwrap();
    symlink(root.join("note"), root.join("absolute")).unwrap();
    for path in ["escape", "absolute"] {
        assert_eq!(
            cap.read_text_revision_bounded(path, 8).unwrap_err().kind,
            FsErrorKind::ConfinementViolation
        );
    }
    assert_eq!(
        ReadOnlyFs::new(&root)
            .unwrap()
            .read_text_revision_bounded("note", 3)
            .unwrap()
            .contents,
        "new"
    );
    let ws = Workspace::new().mount_read_only("project", &root).unwrap();
    assert_eq!(
        ws.read_text_revision_bounded("project/note", 3)
            .unwrap()
            .revision,
        cap.read_text_revision("note").unwrap().revision
    );
    // A sparse huge file must not cause metadata-driven buffering.
    fs::File::create(root.join("huge"))
        .unwrap()
        .set_len(1 << 40)
        .unwrap();
    assert_eq!(
        cap.read_text_revision_bounded("huge", 8).unwrap_err().kind,
        FsErrorKind::ContentTooLarge
    );
    // FIFO opening must not wait for a writer.
    use std::ffi::CString;
    let fifo = CString::new(root.join("fifo").as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert_eq!(
        cap.read_text_revision_bounded("fifo", 8).unwrap_err().kind,
        FsErrorKind::NonRegularFile
    );
    fs::hard_link(root.join("note"), root.join("alias")).unwrap();
    cap.save_text("note", "replacement", None).unwrap();
    assert_eq!(fs::read_to_string(root.join("alias")).unwrap(), "new");
    fs::remove_dir_all(root).unwrap();
}
