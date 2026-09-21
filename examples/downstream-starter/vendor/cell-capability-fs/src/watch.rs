//! Caller-driven, handle-relative filesystem observation.
use crate::{FsError, LocalFs, ReadOnlyFs, Root, Workspace, io_error};
use cap_fs_ext::DirExt;
use cap_std::fs::Dir;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

/// Resource budgets for a recursive snapshot and a single event batch.
#[derive(Clone, Copy, Debug)]
pub struct WatchOptions {
    pub max_entries: usize,
    pub max_events: usize,
}
impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            max_entries: 100_000,
            max_events: 10_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RescanReason {
    ScanFailed,
    EntryLimit,
    EventLimit,
}

/// Logical paths only. Renames are reported as deletion and creation.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WatchEvent {
    Created(String),
    Modified(String),
    Deleted(String),
    RescanRequired(RescanReason),
}

#[derive(PartialEq, Eq)]
struct Fingerprint {
    kind: u8,
    len: u64,
    modified: Option<cap_std::time::SystemTime>,
    link: Option<PathBuf>,
}
type Snapshot = BTreeMap<String, Fingerprint>;

/// A subscription with no background thread. Call `poll` at the desired cadence.
/// Dropping or cancelling releases its retained filesystem authority.
pub struct FsWatch {
    roots: Vec<(String, Arc<Root>)>,
    options: WatchOptions,
    baseline: Option<Snapshot>,
    cancelled: bool,
}

impl LocalFs {
    pub fn watch(&self, options: WatchOptions) -> Result<FsWatch, FsError> {
        FsWatch::new(vec![(String::new(), self.root.clone())], options)
    }
}
impl ReadOnlyFs {
    pub fn watch(&self, options: WatchOptions) -> Result<FsWatch, FsError> {
        self.0.watch(options)
    }
}
impl Workspace {
    pub fn watch(&self, options: WatchOptions) -> Result<FsWatch, FsError> {
        FsWatch::new(
            self.mounts
                .iter()
                .map(|m| (m.name.clone(), m.root.clone()))
                .collect(),
            options,
        )
    }
}

impl FsWatch {
    fn new(roots: Vec<(String, Arc<Root>)>, options: WatchOptions) -> Result<Self, FsError> {
        if options.max_entries == 0 || options.max_events == 0 {
            return Err(FsError {
                kind: crate::FsErrorKind::InvalidPath,
                path: ".".into(),
                operation: "watch".into(),
            });
        }
        let mut watch = Self {
            roots,
            options,
            baseline: None,
            cancelled: false,
        };
        match watch.scan() {
            Ok(snapshot) => watch.baseline = Some(snapshot),
            Err(ScanError::Io(error, path)) => return Err(io_error(error, &path, "watch")),
            Err(ScanError::Limit) => {
                return Err(FsError {
                    kind: crate::FsErrorKind::Other("watch-entry-limit".into()),
                    path: ".".into(),
                    operation: "watch".into(),
                });
            }
        }
        Ok(watch)
    }

    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.roots.clear();
        self.baseline = None;
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    pub fn poll(&mut self) -> Vec<WatchEvent> {
        if self.cancelled {
            return Vec::new();
        }
        let snapshot = match self.scan() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.baseline = None;
                return vec![WatchEvent::RescanRequired(match error {
                    ScanError::Limit => RescanReason::EntryLimit,
                    ScanError::Io(..) => RescanReason::ScanFailed,
                })];
            }
        };
        let Some(previous) = self.baseline.replace(snapshot) else {
            return vec![WatchEvent::RescanRequired(RescanReason::ScanFailed)];
        };
        let current = self.baseline.as_ref().unwrap();
        let mut events = Vec::new();
        for path in previous.keys() {
            if !current.contains_key(path) {
                events.push(WatchEvent::Deleted(path.clone()));
            }
            if events.len() > self.options.max_events {
                return vec![WatchEvent::RescanRequired(RescanReason::EventLimit)];
            }
        }
        for (path, value) in current {
            match previous.get(path) {
                None => events.push(WatchEvent::Created(path.clone())),
                Some(old) if old != value => events.push(WatchEvent::Modified(path.clone())),
                _ => {}
            }
            if events.len() > self.options.max_events {
                return vec![WatchEvent::RescanRequired(RescanReason::EventLimit)];
            }
        }
        events
    }

    fn scan(&self) -> Result<Snapshot, ScanError> {
        let mut snapshot = Snapshot::new();
        for (prefix, root) in &self.roots {
            scan_dir(
                &root.dir,
                prefix,
                &mut snapshot,
                self.options.max_entries,
                0,
            )?;
        }
        Ok(snapshot)
    }
}

enum ScanError {
    Io(std::io::Error, String),
    Limit,
}
fn scan_dir(
    dir: &Dir,
    prefix: &str,
    snapshot: &mut Snapshot,
    limit: usize,
    depth: usize,
) -> Result<(), ScanError> {
    if depth >= 256 {
        return Err(ScanError::Limit);
    }
    let logical = if prefix.is_empty() { "." } else { prefix };
    let fail = |e| ScanError::Io(e, logical.to_owned());
    for entry in dir.entries().map_err(fail)? {
        let entry = entry.map_err(fail)?;
        let name = entry.file_name().into_string().map_err(|_| {
            fail(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid-name",
            ))
        })?;
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if snapshot.len() >= limit {
            return Err(ScanError::Limit);
        }
        let metadata = dir.symlink_metadata(&name).map_err(fail)?;
        let ft = metadata.file_type();
        let kind = if ft.is_symlink() {
            3
        } else if ft.is_dir() {
            2
        } else if ft.is_file() {
            1
        } else {
            4
        };
        let link = if ft.is_symlink() {
            Some(dir.read_link_contents(&name).map_err(fail)?)
        } else {
            None
        };
        snapshot.insert(
            path.clone(),
            Fingerprint {
                kind,
                len: metadata.len(),
                modified: metadata.modified().ok(),
                link,
            },
        );
        if ft.is_dir() {
            // One component, nofollow: a concurrent symlink replacement cannot
            // acquire authority to any target, even one inside the grant.
            let child = dir.open_dir_nofollow(&name).map_err(fail)?;
            scan_dir(&child, &path, snapshot, limit, depth + 1)?;
        }
    }
    Ok(())
}
