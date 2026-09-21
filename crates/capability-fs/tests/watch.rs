use cell_capability_fs::{
    FsError, FsWatch, LocalFs, ReadOnlyFs, RescanReason, WatchEvent, WatchOptions, Workspace,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!(
                "cell-fs-watch-{}-{}",
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

fn watch(kind: Kind, root: &Path, options: WatchOptions) -> Result<FsWatch, FsError> {
    // The watch must retain its own authority after the capability is dropped.
    match kind {
        Kind::Local => LocalFs::new(root)?.watch(options),
        Kind::ReadOnly => ReadOnlyFs::new(root)?.watch(options),
        Kind::Workspace => Workspace::new().mount("grant", root)?.watch(options),
    }
}
fn logical(kind: Kind, path: &str) -> String {
    match kind {
        Kind::Workspace => format!("grant/{path}"),
        _ => path.to_owned(),
    }
}
fn contains(events: &[WatchEvent], expected: WatchEvent) {
    assert!(
        events.contains(&expected),
        "missing {expected:?} in {events:?}"
    );
}
fn rescan_only(events: Vec<WatchEvent>, reason: RescanReason) {
    assert_eq!(events, vec![WatchEvent::RescanRequired(reason)]);
}

#[test]
fn initial_snapshot_then_recursive_create_modify_delete_and_rename() {
    for kind in KINDS {
        let root = TempDir::new();
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("nested/existing"), "old").unwrap();
        let mut watcher = watch(kind, &root.0, WatchOptions::default()).unwrap();
        assert!(!watcher.is_cancelled());
        assert!(
            watcher.poll().is_empty(),
            "initial entries are baseline, not creations"
        );

        fs::write(root.join("nested/new"), "new").unwrap();
        contains(
            &watcher.poll(),
            WatchEvent::Created(logical(kind, "nested/new")),
        );
        // Different lengths avoid relying on platform-specific mtime resolution.
        fs::write(root.join("nested/existing"), "substantially longer").unwrap();
        contains(
            &watcher.poll(),
            WatchEvent::Modified(logical(kind, "nested/existing")),
        );
        fs::rename(root.join("nested/new"), root.join("nested/renamed")).unwrap();
        let events = watcher.poll();
        contains(&events, WatchEvent::Deleted(logical(kind, "nested/new")));
        contains(
            &events,
            WatchEvent::Created(logical(kind, "nested/renamed")),
        );
        fs::remove_file(root.join("nested/renamed")).unwrap();
        contains(
            &watcher.poll(),
            WatchEvent::Deleted(logical(kind, "nested/renamed")),
        );
        assert!(watcher.poll().is_empty());
    }
}

#[test]
fn newly_created_directories_are_scanned_recursively() {
    let root = TempDir::new();
    let mut watcher = watch(Kind::Local, &root.0, WatchOptions::default()).unwrap();
    fs::create_dir_all(root.join("a/b")).unwrap();
    fs::write(root.join("a/b/file"), "contents").unwrap();
    let events = watcher.poll();
    for path in ["a", "a/b", "a/b/file"] {
        contains(&events, WatchEvent::Created(path.into()));
    }
}

#[test]
fn workspace_watches_every_mount_including_readonly_with_prefixes() {
    let first = TempDir::new();
    let second = TempDir::new();
    let workspace = Workspace::new()
        .mount("first", &first.0)
        .unwrap()
        .mount_read_only("second", &second.0)
        .unwrap();
    let mut watcher = workspace.watch(WatchOptions::default()).unwrap();
    drop(workspace);
    fs::write(first.join("same"), "one").unwrap();
    fs::write(second.join("same"), "two").unwrap();
    let events = watcher.poll();
    contains(&events, WatchEvent::Created("first/same".into()));
    contains(&events, WatchEvent::Created("second/same".into()));
    assert!(!events.contains(&WatchEvent::Created("same".into())));
}

#[test]
fn cancellation_is_idempotent_and_polls_remain_empty() {
    for kind in KINDS {
        let root = TempDir::new();
        let mut watcher = watch(kind, &root.0, WatchOptions::default()).unwrap();
        fs::write(root.join("pending"), "not polled").unwrap();
        watcher.cancel();
        watcher.cancel();
        assert!(watcher.is_cancelled());
        assert!(watcher.poll().is_empty());
        fs::write(root.join("later"), "later").unwrap();
        assert!(watcher.poll().is_empty());
        drop(watcher);
    }
}

#[test]
fn transient_changes_have_no_event_history_guarantee() {
    let root = TempDir::new();
    let mut watcher = watch(Kind::Local, &root.0, WatchOptions::default()).unwrap();
    fs::write(root.join("transient"), "gone before poll").unwrap();
    fs::remove_file(root.join("transient")).unwrap();
    // Directory metadata may change; the transient file itself must not appear.
    for event in watcher.poll() {
        match event {
            WatchEvent::Created(path) | WatchEvent::Modified(path) | WatchEvent::Deleted(path) => {
                assert_ne!(path, "transient")
            }
            other => panic!("unexpected rescan: {other:?}"),
        }
    }
}

#[test]
fn event_limit_replaces_the_entire_partial_batch() {
    for kind in KINDS {
        let root = TempDir::new();
        let mut watcher = watch(
            kind,
            &root.0,
            WatchOptions {
                max_entries: 128,
                max_events: 1,
            },
        )
        .unwrap();
        for path in ["a", "b", "c"] {
            fs::write(root.join(path), "new").unwrap();
        }
        rescan_only(watcher.poll(), RescanReason::EventLimit);
    }
}

#[test]
fn entry_limit_replaces_partial_events_and_construction_errors_are_typed() {
    for kind in KINDS {
        let root = TempDir::new();
        let mut watcher = watch(
            kind,
            &root.0,
            WatchOptions {
                max_entries: 4,
                max_events: 128,
            },
        )
        .unwrap();
        for index in 0..16 {
            fs::write(root.join(&format!("file-{index}")), "new").unwrap();
        }
        rescan_only(watcher.poll(), RescanReason::EntryLimit);
        let error: FsError = match watch(
            kind,
            &root.0,
            WatchOptions {
                max_entries: 4,
                max_events: 128,
            },
        ) {
            Ok(_) => panic!("over-limit initial snapshot must fail"),
            Err(error) => error,
        };
        assert_eq!(error.operation, "watch");
        assert!(!format!("{error:?} {error}").contains(root.0.to_str().unwrap()));
    }
}

#[cfg(unix)]
#[test]
fn symlinks_are_observed_but_never_traversed() {
    use std::os::unix::fs::symlink;
    for kind in KINDS {
        let root = TempDir::new();
        let outside = TempDir::new();
        fs::create_dir(root.join("inside")).unwrap();
        fs::write(outside.join("secret"), "old").unwrap();
        symlink(&outside.0, root.join("escape")).unwrap();
        symlink("inside", root.join("alias")).unwrap();
        symlink("missing", root.join("dangling")).unwrap();
        symlink("cycle", root.join("cycle")).unwrap();
        let mut watcher = watch(kind, &root.0, WatchOptions::default()).unwrap();
        fs::write(outside.join("secret"), "different length outside").unwrap();
        fs::write(outside.join("new"), "outside").unwrap();
        assert!(watcher.poll().is_empty());
        fs::write(root.join("inside/file"), "inside").unwrap();
        let events = watcher.poll();
        contains(&events, WatchEvent::Created(logical(kind, "inside/file")));
        assert!(!events.contains(&WatchEvent::Created(logical(kind, "alias/file"))));
        fs::remove_file(root.join("dangling")).unwrap();
        symlink("another-missing-target", root.join("dangling")).unwrap();
        contains(
            &watcher.poll(),
            WatchEvent::Modified(logical(kind, "dangling")),
        );
        fs::remove_file(root.join("escape")).unwrap();
        contains(
            &watcher.poll(),
            WatchEvent::Deleted(logical(kind, "escape")),
        );
        assert_eq!(
            fs::read_to_string(outside.join("secret")).unwrap(),
            "different length outside"
        );
    }
}

#[cfg(unix)]
#[test]
fn replacing_a_subdirectory_with_an_outside_link_does_not_scan_outside() {
    use std::os::unix::fs::symlink;
    for kind in KINDS {
        let root = TempDir::new();
        let outside = TempDir::new();
        fs::create_dir(root.join("child")).unwrap();
        fs::write(root.join("child/old"), "inside").unwrap();
        fs::write(outside.join("secret"), "outside").unwrap();
        let mut watcher = watch(kind, &root.0, WatchOptions::default()).unwrap();
        fs::remove_dir_all(root.join("child")).unwrap();
        symlink(&outside.0, root.join("child")).unwrap();
        let events = watcher.poll();
        contains(&events, WatchEvent::Deleted(logical(kind, "child/old")));
        contains(&events, WatchEvent::Modified(logical(kind, "child")));
        assert!(!events.contains(&WatchEvent::Created(logical(kind, "child/secret"))));
        fs::write(outside.join("secret"), "changed outside length").unwrap();
        assert!(watcher.poll().is_empty());
    }
}

#[cfg(unix)]
#[test]
fn root_path_replacement_does_not_retarget_watch_authority() {
    for kind in KINDS {
        let temp = TempDir::new();
        fs::create_dir(temp.join("root")).unwrap();
        let mut watcher = watch(kind, &temp.join("root"), WatchOptions::default()).unwrap();
        fs::rename(temp.join("root"), temp.join("retained")).unwrap();
        fs::create_dir(temp.join("root")).unwrap();
        // Consume any root metadata changes caused by the host rename.
        let _ = watcher.poll();
        fs::write(temp.join("root/decoy"), "replacement root").unwrap();
        assert!(watcher.poll().is_empty());
        fs::write(temp.join("retained/real"), "retained grant").unwrap();
        contains(&watcher.poll(), WatchEvent::Created(logical(kind, "real")));
    }
}

#[cfg(unix)]
#[test]
fn scan_failure_clears_baseline_and_recovery_requests_rescan() {
    use std::os::unix::fs::PermissionsExt;
    for kind in KINDS {
        let root = TempDir::new();
        fs::create_dir(root.join("blocked")).unwrap();
        fs::write(root.join("blocked/old"), "old").unwrap();
        let mut watcher = watch(kind, &root.0, WatchOptions::default()).unwrap();
        let original = fs::metadata(root.join("blocked")).unwrap().permissions();
        fs::set_permissions(root.join("blocked"), fs::Permissions::from_mode(0)).unwrap();
        // Privileged processes can bypass mode bits; do not assert a failure they cannot induce.
        let denied = fs::read_dir(root.join("blocked")).is_err();
        let failed_poll = if denied { Some(watcher.poll()) } else { None };
        let initial_error = if denied {
            Some(match watch(kind, &root.0, WatchOptions::default()) {
                Ok(_) => panic!("unreadable initial snapshot must fail"),
                Err(error) => error,
            })
        } else {
            None
        };
        fs::set_permissions(root.join("blocked"), original).unwrap();
        if !denied {
            continue;
        }
        rescan_only(failed_poll.unwrap(), RescanReason::ScanFailed);
        let error: FsError = initial_error.unwrap();
        assert_eq!(error.operation, "watch");
        assert!(!format!("{error:?} {error}").contains(root.0.to_str().unwrap()));
        fs::remove_file(root.join("blocked/old")).unwrap();
        fs::write(root.join("blocked/new"), "new baseline").unwrap();
        rescan_only(watcher.poll(), RescanReason::ScanFailed);
        assert!(watcher.poll().is_empty());
        fs::write(root.join("blocked/new"), "longer after recovery").unwrap();
        contains(
            &watcher.poll(),
            WatchEvent::Modified(logical(kind, "blocked/new")),
        );
    }
}
