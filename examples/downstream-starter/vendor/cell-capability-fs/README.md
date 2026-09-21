# Filesystem capabilities

`cell-capability-fs` exposes explicitly granted host directories through three APIs:

- `LocalFs`: a readable, writable directory grant.
- `ReadOnlyFs`: a readable directory grant; mutation methods return access-denied errors.
- `Workspace`: named directory grants, each independently writable or read-only. Guest paths such as `project/src/main.rs` select a mount; the workspace root lists the mounts.

## Confinement contract

Filesystem operations use **cap-std 4 directory handles retained from construction** (or mount creation), not repeated resolution of the original host pathname. Renaming the root and placing another directory at its former pathname does not retarget the capability. The host-facing `root()` path is descriptive, not the authority used to perform operations.

Guest paths must remain relative to their grant. Absolute paths, parent-directory traversal in guest paths, and Windows path prefixes are rejected. A workspace mount is its own grant: a symlink cannot escape it into another mount.

Operations that follow symlinks support **confined relative targets**, including relative targets containing `..` when resolution stays inside the grant. Absolute symlink targets are rejected even when they name a location inside the granted directory. Both intermediate symlinks and final symlinks are subject to confinement when followed. In particular, `write_text` must neither truncate an existing outside target nor create a dangling outside target through a symlink.

`remove_file` unlinks a final symlink itself, rather than its target. Its intermediate path components must still resolve inside the grant.

## Nonfollowing inspection

All three APIs provide:

```rust,ignore
pub fn symlink_metadata(&self, path: impl AsRef<str>) -> Result<Metadata, FsError>
```

`Metadata.file_type` distinguishes `File`, `Directory`, `Symlink`, and `Other`. Inspection does not follow the final component, so dangling symlinks and cyclic symlinks can be reported as `Symlink`. Intermediate components are still resolved under the confinement rules. This is type inspection, not an object-identity or snapshot guarantee.

`list_dir` likewise reports each entry's nonfollowing file type. A caller walking a tree without following symlinks should recurse only into entries reported as directories. Concurrent changes can make a previously inspected path stale; inspection is not a substitute for confinement on each subsequent operation.

Guest operation errors use guest-visible paths or generic error context, not host root paths or resolved host targets. Construction and mount errors are also typed and sanitized: their logical paths are `.` and the mount name respectively, not the configured host directory. Linker registration errors remain runtime/linker errors (`anyhow`), not filesystem operation errors.

## Limits of the grant

Directory handles confine path resolution; they do **not** prevent a trusted host from moving an already opened directory. A capability continues to refer to the directory it opened, even if the host subsequently moves it elsewhere. This is an authority grant to that directory, not a promise about its permanent location in the host namespace.

Preexisting hardlinks and mounts within a granted directory are part of the grant. Confinement does not isolate their underlying storage or undo authority the host has already made reachable there. A read-only grant prevents mutations through this API, not writes through other host handles or capabilities.

`write_text` creates or truncates a file and has no replacement guarantee. Use `save_text` for existing-file replacement, as specified below; there is still no general open or rename API.

## Existing-file save

All native capabilities and `cell_guest::fs` expose `read_text_revision(path)` and
`save_text(path, contents, expected_revision)`. The read returns `{ contents,
revision }`; save returns `Saved(new_revision)` or `Conflict(current_revision)`
inside the ordinary filesystem `Result`. `None` skips conflict detection, not
safe replacement. Scope can use these operations with logical capability paths
without canonicalizing or writing host paths.

### Confinement and symlinks

Save resolves and retains the parent directory through the grant's `cap-std`
handle, then creates, writes, renames and cleans up through that same handle.
Intermediate relative symlinks are allowed only within the grant; absolute or
escaping intermediate links are rejected. The existing final entry must be a
regular file: final symlinks (including contained, dangling and outside links),
directories and special files are rejected with `InvalidPath`. Missing targets
return `NotFound`; this is not a create-file API.

These type and existence checks are not namespace locks. If an external actor
changes the final entry after inspection, rename replaces the entry then present
(including a newly inserted symlink), never writes through that symlink. A target
removed after inspection may be recreated by rename. Opened parent directories
retain the authority and host-namespace limitations described above. Concurrent
host manipulation of temporary entries in a writable directory is not isolated;
this API is not a transaction against a hostile host sharing the directory.

### Replacement and platforms

On Unix (including Linux and macOS), a same-directory temporary regular file is
fully written and closed, then installed with a single confined rename. On local
filesystems providing POSIX rename semantics, the directory entry switches
atomically from old to new: readers opening the destination see the old or new
file, not a partly written new file, and existing open handles keep the old file.
There is no delete-then-rename fallback. Filesystem/server implementations that
do not honor POSIX rename semantics cannot be given this guarantee (notably
some network or unusual filesystems).

Non-Unix platforms, including Windows, currently return
`Other("unsupported-save-platform")` without changing files. This deliberately
avoids promising equivalent replacement semantics without a verified confined
backend. The rest of the filesystem API remains available there.

### Permissions and metadata

A read-only capability or mount denies save. Save also explicitly denies a
regular target whose Unix mode has no write bits, even when directory permission
would otherwise permit rename. Other authorization is enforced by the OS:
creation and replacement require appropriate parent-directory access. It does
not require an in-place writable target handle, so a writable parent may permit
replacement where opening the old file for writing would fail.

The temporary file is exclusively created with mode `0600` (subject to umask),
then receives the inspected target's ordinary Unix permission bits (`0777`)
before installation. Setuid, setgid and sticky bits are cleared.
The replacement is a new inode owned by the creating process; group assignment
and default ACL behavior follow the OS. Ownership, ACLs, extended attributes,
timestamps and hardlink identity are **not preserved**. Existing hardlinks retain
the old inode. Do not rely on save to preserve security metadata or ownership.

### Durability and failure

**No crash/power-loss durability is promised.** Neither file data nor directory
metadata is fsynced; write and close followed by rename provide visibility, not
stable-storage persistence. Success does not promise survival across a crash.
There are no post-rename fallible persistence operations. Before rename, failures
leave the destination unchanged by Cell, and temporary files are removed on a
best-effort basis. Cleanup can itself fail; process termination/crashes can leave
`.cell-save-*` files. No automatic orphan scavenging is performed.

### Optional conflict detection (independent of replacement)

Tokens are version-prefixed SHA-256 digests of file bytes, containing no host
paths, timestamps, inode numbers or other host metadata. Treat them as opaque;
they are content fingerprints, not secrets or authentication credentials. Equal
bytes have equal tokens, including across paths. They reveal content equality
and permit guessing known content; do not expose them as secret-bearing data.
`read_text_revision` computes its token from the exact returned UTF-8 bytes.
After save, the returned token describes the submitted bytes, not a later reread.

When an expected token is supplied, save reads the current file and compares its
content token before preparing replacement. A mismatch returns structured
`Conflict(current_revision)` without modifying the target or creating a temp.
Any malformed/unknown token also mismatches. Changes to metadata only, or changes
followed by restoration of identical bytes (ABA), are not conflicts. Read errors
and missing files remain filesystem errors, not content conflicts.

This is **best-effort optimistic detection, not atomic compare-and-swap**: an
external writer can change the file during the read or between comparison and
rename, and its update can be overwritten. Concurrent in-place writes can also
produce a mixed read. Callers requiring lossless coordination must arrange
external synchronization. Atomic rename neither detects conflicts nor closes
this race, and is used identically with and without an expected revision.

## Native polling watches

`LocalFs`, `ReadOnlyFs`, and `Workspace` provide the native Rust API
`watch(WatchOptions) -> Result<FsWatch, FsError>`. This API is not exposed through
WIT or `cell_guest::fs`; it does not change the component ABI.

```rust,ignore
use cell_capability_fs::{LocalFs, WatchEvent, WatchOptions};

let filesystem = LocalFs::new("./project")?;
let mut watch = filesystem.watch(WatchOptions::default())?;
// Call periodically from your own event loop; poll does not wait for changes.
for event in watch.poll() {
    match event {
        WatchEvent::Created(path) => println!("created: {path}"),
        WatchEvent::Modified(path) => println!("modified: {path}"),
        WatchEvent::Deleted(path) => println!("deleted: {path}"),
        WatchEvent::RescanRequired(reason) => {
            // Invalidate cached state and refresh it through the capability.
            println!("refresh required: {reason:?}");
        }
        _ => {} // Allow future event variants.
    }
}
watch.cancel();
assert!(watch.is_cancelled());
assert!(watch.poll().is_empty());
```

A watch covers the **whole granted tree**, recursively. A workspace watch covers
all its mounts, including read-only mounts, and reports mount-prefixed logical
paths such as `project/src/main.rs`. Read-only watches observe changes made by
other host writers without granting write authority. Watch authority is retained
independently of the capability object and remains tied to the opened directory
handles, not the original host path. Replacing that path does not retarget a watch.

Construction takes the initial snapshot; existing entries are not emitted as
creations. Initial scan failures return a typed, sanitized `FsError` with operation
`watch`. `FsWatch::poll()` synchronously scans and returns `Vec<WatchEvent>`;
`Created(String)`, `Modified(String)`, and `Deleted(String)` describe differences
from the previous complete snapshot. Renames are deletion plus creation, with no
rename pairing. Do not rely on event order or a particular set of directory
modification events.

`WatchOptions { max_entries: usize, max_events: usize }` implements `Default`.
Choose explicit limits when your application needs a particular resource budget.
Zero budgets are rejected. Traversal also has a fixed depth limit of 256 to bound
stack and open-handle use; exceeding it is reported as `EntryLimit` (or
`Other("watch-entry-limit")` during construction). Snapshots and returned event batches are bounded. A scan exceeding the entry
limit returns only `RescanRequired(RescanReason::EntryLimit)`; an overflowing event
batch is replaced in its entirety by
`RescanRequired(RescanReason::EventLimit)`, never a misleading partial batch.
An initial snapshot exceeding its entry budget fails construction. A poll scan
failure returns `RescanRequired(RescanReason::ScanFailed)` and clears the baseline.
The next successful scan resets the baseline and also returns only
`RescanRequired(RescanReason::ScanFailed)`, rather than invented create/delete
history across the gap. Rescan notifications mean the caller must refresh its
own cached state; they are not a complete change list.

### Confinement, lifecycle, and limitations

Scanning uses handle-relative, nonfollowing directory operations. **No symlinks
are traversed**, including confined directory links, dangling links, cycles, and
links into other workspace mounts. The link itself is observed using its own
metadata and target string; target contents are not scanned through the link.
The grant's existing hardlink, mount, and host-movement limitations still apply.
A scan is not an atomic snapshot against concurrent host namespace changes.

Fingerprints use metadata type, length, modification time, and symlink target
string, **not file contents**. Filesystem/platform modification-time resolution
and update behavior differ; even same-length writes may be detected differently
across platforms. Changes with identical fingerprint metadata can be missed.
Entries created and deleted between polls can disappear without any event, and
multiple intervening changes can collapse into one difference. A complete batch
contains at most one event per logical path, and unchanged consecutive snapshots
produce no duplicates. Repeated real changes or repeated rescan failures can
produce the same event across polls; callers should handle events idempotently.
There is **no
event-history guarantee**, OS notification stream, or real-time delivery promise.
Polling cost depends on the size of the granted trees; schedule polls explicitly
and choose budgets and frequency appropriate to your workload.

`cancel()` releases the watch's authority and snapshot, is idempotent, and makes
`is_cancelled()` true; all later polls return an empty vector. Dropping a watch
unsubscribes/releases its state. Watches create no background threads and need
no worker shutdown or join.

## Regression tests

`tests/confinement.rs` covers local, read-only, and workspace grants; outside and dangling symlink writes; intermediate escapes; final-link unlinking; confined relative symlinks; absolute-target rejection; nonfollowing metadata and listings; and root-path replacement after construction.

`tests/watch.rs` specifies initial snapshots, recursive create/modify/delete and
rename observation, retained watch authority, cancellation, read-only and
mount-prefixed workspace observation, bounded entry/event batches, and polling
limitations. Unix-only cases cover nontraversed symlinks, directory-link and root
path replacement, and scan failure/recovery. Permission-failure assertions are
skipped when the process can bypass mode bits. Modification tests change file
length instead of depending on platform mtime precision. Run with
`cargo test -p cell-capability-fs --test watch`.

The confinement tests are Unix-gated because they use Unix symlinks. Temporary directories use only the standard library, with process IDs and an atomic counter for unique names and cleanup on drop.

Run from the repository root:

```sh
cargo test -p cell-capability-fs --test confinement
```

## Typed errors and compatibility

All fallible filesystem methods on `LocalFs`, `ReadOnlyFs`, and `Workspace`, and
all `cell_guest::fs` methods other than `exists`, return `Result<T, FsError>`.
`FsError` implements `Display` and `Error` and contains `kind: FsErrorKind`,
`path: String`, and `operation: String`. The path is the caller's original
logical capability path, including the mount prefix for workspace calls.
Neither OS error messages, host roots, resolved symlink targets, nor OS error
codes are returned. A caller-supplied path is echoed; this is not a filter for
sensitive text supplied by the caller itself.

Categories are:

- `PermissionDenied`: a read-only policy denial or ordinary OS permissions.
- `NotFound`: missing entries or workspace mounts.
- `InvalidPath`: empty paths where an entry is required, NUL bytes, invalid
  mount names, or backend invalid-input errors.
- `ConfinementViolation`: rejected absolute/traversing paths or backend symlink
  escape attempts.
- `InvalidTextEncoding`: non-UTF-8 file contents or directory entry names.
- `Io`: other filesystem failures, including existing directories, wrong entry
  types, nonempty directory removal, and symlink loops.
- `Other(String)`: a category not known to this version of the Rust API.

Operation strings are `read-text`, `write-text`, `symlink-metadata`, `list-dir`,
`create-dir`, `remove-file`, and `remove-dir`; configuration uses `open-root`
and `mount`. New operation names may be introduced.

```rust,ignore
use cell_capability_fs::{FsErrorKind, LocalFs};

fn read_optional(fs: &LocalFs, path: &str) -> Result<Option<String>, cell_capability_fs::FsError> {
    match fs.read_text(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) => match &error.kind {
            FsErrorKind::NotFound => Ok(None),
            FsErrorKind::PermissionDenied => {
                eprintln!("access denied for {} during {}", error.path, error.operation);
                Err(error)
            }
            _ => Err(error), // Required: categories are non-exhaustive.
        },
    }
}
```

Both Rust error types are `#[non_exhaustive]`; consumers should match categories
with a fallback rather than parsing display text. WIT uses an `error-category`
variant with `other(string)` and an `fs-error` record. Future categories can use
`other` without adding variant cases and changing the component ABI. Host and
guest maintain independent Rust error types with the same public contract;
wire conversion preserves unknown strings and logical context.

This replaces the previous string-error API and WIT ABI. Rebuild host and guest
components together (including downstream copies of `cell:fs@0.1.0` WIT);
old compiled components are not compatible. The three in-tree WIT copies are
checked for equality by a regression test.

### Backend classification and preserved behavior

cap-primitives 4's escape marker is private, with no public downcastable error
category. Classification recognizes only its exact synthetic `PermissionDenied`
message `a path led outside of the filesystem` when `raw_os_error()` is absent.
Ordinary EACCES/EPERM and other permission errors are **not** assumed to be escapes.
A backend change to that private marker may degrade classification to
`PermissionDenied`, but does not weaken handle-relative confinement. Review this
adapter when upgrading cap-std; symlink regression tests exercise the marker.

`exists` remains a boolean: inaccessible, invalid, escaping, and missing paths
return false. Existing root behavior is unchanged: local `exists(".")` is false;
workspace `exists(".")` and existing mount roots are true. Read-only mutations
retain their existing permission-check precedence, and normalization, sorted
listings, final-symlink inspection/unlinking, create/truncate behavior, and
retained-root handles are unchanged.

`tests/errors.rs` covers category/context behavior and ordinary OS permissions.
Non-UTF-8 directory-name tests run on Unix except macOS, whose filesystems reject
those names at creation. The existing confinement suite includes race tests and
Unix socket type inspection.
