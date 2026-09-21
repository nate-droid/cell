# Building Projects with Cell

Cell runs WebAssembly components with no host capabilities installed by default. A guest declares imports in WIT; a host explicitly installs matching capability implementations. Downstream guests should use those Cell capabilities rather than host-based APIs such as Rust's `std::fs`, which require ambient WASI-style host authority that Cell does not install by default. For the high-level overview, see the [README](README.md).

## Start a standalone downstream project

Use the [downstream host + guest starter](examples/downstream-starter/README.md)
for a project outside Cell's workspace. This is an interim **local-checkout
workflow**, not a registry release or a substitute for the
[downstream dependency policy](README.md#downstream-dependencies). Canonical
release coordinates remain maintainer decisions. Review the checkout's
[LICENSE](LICENSE) before redistributing staged inputs.

You need Python 3.11+, a Rust toolchain compatible with the checkout's Wasmtime
version, and the guest target:

```sh
rustup target add wasm32-unknown-unknown
```

From the **Cell repository root**, build and run the starter with:

```sh
python3 examples/downstream-starter/run.py . --offline
```

`--offline` requires cached third-party dependencies. Omit it to allow builds to
download dependencies from your configured Cargo registry; package staging itself
is always offline. No network authority is granted to the guest.

To develop elsewhere, copy the **entire** `examples/downstream-starter/` directory
into your project, then run this from the copied starter directory:

```sh
python3 run.py /absolute/path/to/cell --offline
```

The argument is the Cell checkout containing its root `Cargo.toml` and
`Cargo.lock`, not your downstream project. If Python reports it cannot open
`run.py`, check your working directory and the script path: the checked-in folder
is `examples/downstream-starter`, not `examples/downstream`. Copying only the
Python script is insufficient; it needs the adjacent host, guest, and manifests.

The script stages Cargo-listed Cell package inputs into the starter's `vendor/`
and automatically copies matching logging/filesystem WIT into
`guest/wit/deps/`. The starter has its own Cargo workspace and does not depend on
Cell's source directories. Re-running replaces staged packages and generated WIT;
do not edit those generated directories. Keep your generated `Cargo.lock` and
record the Cell revision and any local changes for reproducibility.

The host explicitly converts the guest's core module with embedded WIT metadata
into a validated component. It grants only `StdoutLog` and `ReadOnlyFs`. Expected
output:

```text
downstream guest started
read=hello from the downstream fixture; write=permission-denied
```

This is success: the guest can log and read `input.txt`, but its attempted write
is rejected and handled. It does not indicate a failed build.

### Use your own files and change the guest

From the starter directory, edit `guest/src/lib.rs`, rebuild, then run against a
directory containing your own `input.txt`:

```sh
cargo build -p downstream-guest --target wasm32-unknown-unknown --target-dir target --offline
cargo run -p downstream-host --target-dir target --offline -- target/wasm32-unknown-unknown/debug/downstream_guest.wasm /absolute/path/to/read-only-root
```

`--target-dir target` matches `run.py` even if your environment or Cargo
configuration selects a different build directory. These commands assume you
have already staged dependencies with `run.py`.

The starter host expects the core module produced by this guest build, not an
already-encoded component. Running `run.py` again rebuilds the guest but uses its
generated fixture under `target/read-only-root`, not your custom root. Wasmtime 47
performs canonical ABI post-return cleanup automatically.

To validate the starter in a fresh, isolated workspace, run `make
downstream-starter` from the Cell repository root, or `python3 smoke.py
/absolute/path/to/cell --offline` from the starter directory. The smoke test checks
workspace isolation, matching WIT, successful reads, denied writes, and host error
handling. See the [starter README](examples/downstream-starter/README.md) for
additional prerequisites and troubleshooting.

## Build the examples

From the repository root, install the guest target, build the native runtime/host, then run the smoke check:

```sh
rustup target add wasm32-unknown-unknown
cargo build -p cell -p hello-host --locked
make smoke
```

For a first successful run, use `make hello`: it builds `hello` for `wasm32-unknown-unknown` and runs `hello-host`, which encodes the module as a component using its embedded WIT metadata. Expected output: `hello from a Cell guest`. You can also run `cargo run -p hello-host -- <path>` with a component or a core module containing WIT metadata.

Do not use plain `cargo build` as a native workspace build: component guest packages such as `formatter-provider` need a WebAssembly target and can fail native linking. The quickstart does not require `wasm32-wasip2`, `cargo-component`, or `wasm-tools`.

`make smoke` builds both guests for `wasm32-unknown-unknown`, runs the hello host, and runs `cargo test -p cell`.

You can build either guest separately:

```sh
cargo build -p hello --target wasm32-unknown-unknown
cargo build -p fs-demo --target wasm32-unknown-unknown
```

`target/wasm32-unknown-unknown/debug/hello.wasm` is a core module with embedded WIT metadata; `hello-host` encodes it as a component. Building `hello` for `wasm32-wasip2` produces a component but may introduce WASI imports from Rust's standard library, so it is not the logging-only quick-start path. `target/wasm32-unknown-unknown/debug/fs_demo.wasm` is a core Wasm module; the `cell` integration tests embed its WIT metadata and encode it as a component before loading it. Do not pass the raw `fs_demo.wasm` directly to `Cell::component_from_bytes`.

### Build an explicit fs-demo component

To produce a component file for a host to load, run from the repository root:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-tools --locked
make fs-demo-component
```

The target builds the debug guest, embeds `examples/fs-demo/wit` with world
`fs-demo`, encodes the module as a component, and validates the result. It writes
`target/fs-demo.component.wasm`; pass that file's bytes to
`Cell::component_from_bytes`. `target/fs-demo.metadata.wasm` is an intermediate
core module, not a runnable component. No WASI adapter is needed because this
guest targets `wasm32-unknown-unknown` and imports the Cell filesystem capability
rather than WASI.

The equivalent explicit commands are:

```sh
cargo build -p fs-demo --target wasm32-unknown-unknown
wasm-tools component embed examples/fs-demo/wit \
  target/wasm32-unknown-unknown/debug/fs_demo.wasm \
  --world fs-demo -o target/fs-demo.metadata.wasm
wasm-tools component new target/fs-demo.metadata.wasm \
  -o target/fs-demo.component.wasm
wasm-tools validate target/fs-demo.component.wasm
```

This mirrors the integration tests' `wit-parser` / `wit-component` conversion
(currently version `0.241.2` in `crates/cell/Cargo.toml`), without adding a utility
crate or workspace member. `make smoke` remains independent of the `wasm-tools`
CLI: its tests build and convert the fixture in memory, and do not write the
component file above. Override the CLI location with
`make fs-demo-component WASM_TOOLS=/path/to/wasm-tools` if needed. These commands
assume Cargo's default `target/` directory and debug profile.

Building a component does not grant it filesystem access. The host must install
`cell-capability-fs` with an explicit root and permissions before instantiation;
see [Choose host capabilities](#choose-host-capabilities).

## Run a component from a host

For a host crate under `examples/` in this repository, add dependencies like this, adjusting paths for your crate's location:

```toml
[dependencies]
anyhow = "1"
cell = { path = "../../crates/cell" }
cell-capability-log = { path = "../../crates/capability-log" }
wit-component = "0.241.2"
```

For a separate project, follow the [downstream dependency policy](README.md#downstream-dependencies): use the canonical Git URL with a full commit pin once that URL is designated, not sibling paths. No canonical remote or registry release is configured in this checkout yet. For an executable interim local-checkout workflow, use [the standalone starter](#start-a-standalone-downstream-project), which stages package inputs instead of depending on sibling source paths.

After building `hello`, a host binary run from the repository root can execute it with:

```rust
use cell::Cell;
use cell_capability_log::StdoutLog;

fn main() -> anyhow::Result<()> {
    let cell = Cell::new()?.with(StdoutLog)?;
    let bytes = std::fs::read("target/wasm32-unknown-unknown/debug/hello.wasm")?;
    let bytes = wit_component::ComponentEncoder::default()
        .module(&bytes)?
        .validate(true)
        .encode()?;
    let component = cell.component_from_bytes(bytes)?;
    let mut store = cell.store(());
    let instance = cell.instantiate(&mut store, &component)?;
    let run = instance.get_typed_func::<(), ()>(&mut store, "run")?;
    run.call(&mut store, ())?;
    Ok(())
}
```

`Cell::new()?` creates an empty linker with `()` as its store-state type. For custom host state, use `Cell::<YourState>::empty()?` and `cell.store(your_state)`. The typed export call depends on the guest's WIT exports. The `std::fs::read` call above is host-side code for loading the component bytes; it does not provide filesystem access inside the guest.

If a guest imports an interface the host has not installed, instantiation fails. Cell does not implicitly provide WASI, even when the guest was built for `wasm32-wasip2`.

### Managed execution and invocation isolation

Compile once with `cell.component_from_bytes(bytes)?` (or load a verified artifact), then optionally use `cell.check(&component)?` to validate linker compatibility without executing guest code. Compilation validates the component; linking does not guarantee startup succeeds.

For independent invocations, `Cell::execute` creates a fresh managed store and instance and drops the store on success or failure:

```rust
use cell::{ExecutionErrorKind, ExecutionPolicy};

let policy = ExecutionPolicy {
    fuel: Some(1_000_000),
    memory_bytes: Some(16 * 1024 * 1024),
    ..Default::default()
};
match cell.execute(&component, (), policy, "run", &[]) {
    Ok(results) => println!("Results: {results:?}"),
    Err(error) if error.kind == ExecutionErrorKind::Fuel => {
        eprintln!("Guest exhausted its fuel budget");
    }
    Err(error) => return Err(error.into()),
}
```

Only capabilities explicitly installed on this Cell are available. `ExecutionPolicy::default()` imposes no Cell limits. Fuel is refreshed for startup and each call; call and canonical post-return share a budget. Memory limits apply per linear memory, not to total process memory. Policies also support absolute deadlines, sticky `CancellationToken`s, and logical input/output payload limits. Output limits are checked after results have been allocated and lifted. Native host functions need their own I/O timeouts and cooperative cancellation; guest interruption cannot forcibly stop a blocking host function.

For intentionally stateful repeated calls, use `cell.managed_store(state, policy)`, `cell.instantiate_managed(&mut store, &component)`, and `store.call(&instance, "run", &[])`. Successful calls retain guest and host state. Failures inside Wasmtime (including startup and canonical cleanup) poison the managed store; `store.is_poisoned()` reports this and further execution returns `ExecutionErrorKind::Invalid`. Missing exports and input/output policy validation failures do not themselves poison a store. Canonical cleanup runs as part of Wasmtime 47's successful call path, including before an output-limit error is returned.

**Migration:** existing raw-store and typed-call APIs remain unchanged. If you previously retried a managed store after a guest failure, replace the store and instance, or use `Cell::execute` for every invocation. Supply fresh host state and fresh policies/tokens where needed. Fresh guest stores do not roll back filesystem writes, network requests, or mutable state shared by capability implementations.

### Record execution provenance

Use `Cell::execute_recorded` when an invocation needs a structured record, including on failure. It compiles the supplied bytes and runs in a fresh managed store; existing `execute`, managed-store, and raw-store APIs remain unchanged. No migration is required.

```rust
use cell::{Cell, ExecutionPolicy};

let cell = Cell::new()?; // No capabilities unless explicitly installed.
let bytes = std::fs::read("guest.wasm")?;
let execution = cell.execute_recorded(
    &bytes,
    (),
    ExecutionPolicy {
        fuel: Some(1_000_000),
        memory_bytes: Some(16 * 1024 * 1024),
        ..Default::default()
    },
    "run",
    &[],
);
println!("Component: {}", execution.record.inputs.component_digest);
println!("Outcome: {:?}", execution.record.error_kind);
println!("Elapsed: {:?}", execution.record.observations.elapsed);
let results = execution.result?;
```

`RecordedExecution` contains the normal result and an `ExecutionRecord`:

- **Inputs:** exact component content digest, export, tracked capability identifiers, numeric policy limits, and an optional input-value digest.
- **Outcome:** `error_kind` is `None` on success and otherwise the structured `ExecutionErrorKind`; output digests are available only on success and for supported values.
- **Observations:** wall-clock start, monotonic elapsed duration, absolute deadline, cancellation presence and sampled state, and separately measured startup/call fuel consumption. Fuel budgets reset between phases; call consumption includes canonical cleanup. `None` means a phase never received a budget, while `Some(0)` means measured zero. No memory-consumption metric is claimed; `memory_bytes` records the configured per-linear-memory ceiling.

`values_digest(&[Val])` uses a versioned, type-tagged, length-delimited SHA-256 encoding of dynamic values. Container order and exact floating-point bits are preserved. Resources and unsupported values, including nested ones, yield `None`. These are value digests, not WIT type or canonical ABI digests. Component identity covers the exact supplied bytes (including WAT if supplied), not normalized semantics. For stored artifacts, use `artifacts.resolve(digest)?` and pass the verified bytes to `execute_recorded`; the record computes identity from those same bytes.

**Privacy:** records retain no raw payloads, credentials, capability configuration, host state, or error messages, and Cell does not automatically log them. The separate `result` still contains normal values or diagnostic errors; do not log that wholesale if sensitive. Digests are not encryption and may reveal equality or permit guessing low-entropy inputs. Capability identifiers and export names must also be appropriate for your logging policy.

**Guarantees and limitations:** records separate reproducible execution inputs from timing and cancellation observations, but are not complete replay recipes or authority attestations. Capability identifiers cover successful `Cell::with` installations only; direct linker registrations, mutable authority configuration, engine configuration, host state, and external I/O are not captured. Numeric limits do not make execution deterministic. Metadata never grants permissions. Compilation and hashing are included in elapsed time but are not bounded by guest execution policy. Each recorded call recompiles its supplied bytes; use the existing compiled-component execution API when compilation reuse matters. Host side effects are not rolled back.

### Store and load a component by digest

When your build tool already produces a binary WebAssembly **component**, import
it directly into a local artifact store. Cell does not need to know which tool
produced it, and this workflow does not require `wit-component` in the host:

```rust
use cell::{ArtifactDigest, Cell, LocalArtifactStore};
use cell_capability_log::StdoutLog;

fn main() -> anyhow::Result<()> {
    let artifacts = LocalArtifactStore::new(".cell/artifacts");
    let digest = artifacts.put_file("component.wasm")?;
    println!("{digest}");

    // Persist this identifier; a later execution can parse it without rebuilding.
    let saved: ArtifactDigest = digest.to_string().parse()?;
    let cell = Cell::new()?.with(StdoutLog)?;
    let component = cell.component_from_artifact(&artifacts, saved)?;
    cell.check(&component)?;
    let mut store = cell.store(());
    let instance = cell.instantiate(&mut store, &component)?;
    let run = instance.get_typed_func::<(), ()>(&mut store, "run")?;
    run.call(&mut store, ())?;
    Ok(())
}
```

This example assumes the same `run` export and logging import as `hello`; choose
capabilities and exports to match your component. On a later run, reopen the same
store directory and parse the saved `sha256:<64 lowercase hex digits>` identifier,
then call `component_from_artifact` without importing the file again.

For the core-module build in the preceding example, first use
`ComponentEncoder` as shown there, then call `artifacts.put(bytes)?` with the
encoded component bytes. `put` and `put_file` reject core modules and WAT text;
they check the binary component header, while full WASM validation happens when
Cell compiles the component.

Artifact identity is SHA-256 over the exact bytes. Identical bytes reuse an entry;
changes to the bytes produce a new digest. Publication is atomic and does not
overwrite an existing entry. Loading verifies the digest before compiling those
same bytes, and missing or corrupted entries return errors. The store retains
portable component bytes, not native compiled code, so reuse avoids rebuilding or
reimporting the artifact but does not cache compilation.

A digest establishes byte integrity, not publisher authenticity. Obtain expected
digests through a trusted channel when loading third-party artifacts. Capabilities,
permissions, and execution policy remain host choices: storing a component grants
it no authority. The store is local only, with no registry or dependency resolution.

**Compatibility:** Artifact storage is opt-in. Existing `component_from_bytes`,
capability installation, and instantiation workflows remain unchanged; no migration
is required. The host-side store does not grant filesystem access to guests.

### Diagnose linking and startup failures

Use `cell.check(&component)?` to validate imports against the actual linker before
creating a store or executing guest code. It uses the same linking checks and error
guidance as `Cell::instantiate`, which distinguishes failure to link imports from
failure during instance startup. For recognized missing implementations, the error identifies
the unresolved interface and suggests a matching built-in capability (for
example, `ReadOnlyFs`, `LocalFs`, or `Workspace` for `cell:fs/fs@0.1.0`). A missing
implementation may be a member of an otherwise registered interface; inspect the
underlying error before assuming the entire capability is absent. Missing WASI
imports receive explicit-authority and guest-target guidance.

Other linking failures use neutral guidance: check for incompatible import types,
WIT versions, or an engine mismatch rather than assuming a capability is missing.
Linking checks the actual linker, including direct `linker_mut()` registrations;
the displayed tracked-capability list includes only `.with(...)` installations.
After successful linking, startup failures instead point to traps, host errors,
or resource limits. No capability-installation advice is added to those failures.

The original error chain is preserved. To see both Cell's guidance and the
underlying Wasmtime diagnostic, format the returned error with `format!("{error:#}")`
or `eprintln!("{error:#}")`. Unrecognized Wasmtime linking diagnostics fall back to
neutral guidance rather than guessing the cause.

## Write a guest

A guest needs:

1. A WIT world that declares imports and exports.
2. The WIT packages for imported capabilities under `wit/deps/` or an equivalent WIT dependency setup.
3. Rust bindings generated with `wit-bindgen`.
4. A `cdylib` crate built for a Wasm target.

[`examples/hello/wit/world.wit`](examples/hello/wit/world.wit) defines:

```wit
package cell-example:hello;

world hello {
    import cell:log/log@0.1.0;
    export run: func();
}
```

The imported `cell:log` package is in `examples/hello/wit/deps/cell-log/`. The guest crate uses:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.48"
```

This uses the checkout's `wit-bindgen` version family without workspace
inheritance, so the dependency declaration also works in a standalone guest.

The guest exports its `run` function through generated bindings:

```rust
mod bindings {
    wit_bindgen::generate!({
        world: "hello",
        path: "wit",
        generate_all,
    });

    use super::GuestComponent;
    export!(GuestComponent);
}

struct GuestComponent;

impl bindings::Guest for GuestComponent {
    fn run() {
        bindings::cell::log::log::log("hello from a Cell guest");
    }
}
```

For filesystem guests, `cell-guest` provides `CellPath` and helpers. These are guest-side conveniences, not extra host authority. The host still decides which real directories, if any, the guest can reach.

In Rust guest code, prefer these Cell filesystem bindings over `std::fs` or other WASI-backed filesystem APIs. A guest that calls `std::fs` is asking for host-provided WASI filesystem access, while Cell's intended model is to import `cell:fs/fs@0.1.0` and let the host install a scoped implementation such as `ReadOnlyFs`, `LocalFs`, or `Workspace`.

```rust
use cell_guest::{CellPath, fs};

let input = fs::read_text(CellPath::new("input.txt").map_err(|e| e.to_string())?)
    .map_err(|e| e.to_string())?;
fs::write_text(
    CellPath::new("output.txt").map_err(|e| e.to_string())?,
    &input,
).map_err(|e| e.to_string())?;
```

This snippet assumes the guest function returns `Result<_, String>`. See [`examples/fs-demo`](examples/fs-demo) for a complete filesystem guest.

### Inspect entries and traverse directories

Use `fs::symlink_metadata` to inspect an entry without following its final symlink, rather than opening it and inferring its type from success or failure:

```rust
let metadata = fs::symlink_metadata(
    CellPath::new("input.txt").map_err(|e| e.to_string())?,
).map_err(|e| e.to_string())?;
match metadata.file_type {
    fs::FileType::File => { /* regular file */ }
    fs::FileType::Directory => { /* directory */ }
    fs::FileType::Symlink => { /* skip during non-following traversal */ }
    fs::FileType::Other => { /* socket, device, or another entry type */ }
}
```

Lookup failures are returned as `Err`, not an entry type. Dangling and cyclic final symlinks are reported as `Symlink`. `fs::list_dir` also returns non-following entry types; recursively enumerate only entries reported as `Directory` to avoid directory-symlink cycles. Metadata is not a snapshot: concurrent changes can invalidate a prior observation, and every subsequent operation still enforces confinement independently. Stable filesystem identity is not currently exposed, so identity-based cycle detection for traversal that intentionally follows symlinks is not provided.

Keep downstream filesystem WIT dependencies synchronized with [`crates/capability-fs/wit/fs.wit`](crates/capability-fs/wit/fs.wit), including typed errors, the `metadata` record, `symlink-metadata`, and the revisioned read/save types and functions. The host, guest-generated bindings, and `cell-guest` must agree on the interface definition; the `cell:fs/fs@0.1.0` name alone does not ensure their WIT copies match.

### Save edits to an existing file

Use `fs::read_text_revision` and `fs::save_text` for editing an existing regular
file, rather than truncating it with `write_text`. For a guest function returning
`Result<_, String>`:

```rust
use cell_guest::{CellPath, fs};

let path = CellPath::new("notes.txt").map_err(|e| e.to_string())?;
let original = fs::read_text_revision(&path).map_err(|e| e.to_string())?;
let edited = format!("{}\nUpdated through Cell.\n", original.contents);
match fs::save_text(&path, &edited, Some(&original.revision))
    .map_err(|e| e.to_string())?
{
    fs::SaveOutcome::Saved(revision) => { /* retain the new revision */ }
    fs::SaveOutcome::Conflict(current_revision) => { /* reload and reconcile */ }
}
```

A conflict is a successful structured outcome, not an `FsError`. Revisions are
opaque content fingerprints, not file identities or secrets. Passing `None`
skips conflict detection but still uses the same safe replacement. Detection is
**best-effort optimistic checking, not atomic compare-and-swap**: another writer
can change the file between comparison and replacement. Arrange external
synchronization when lost updates must be prevented.

Save requires an existing regular file and writable authority; it does not
create missing files and rejects final symlinks. On Unix it prepares a
same-directory temporary file and replaces the destination with a confined
rename, providing old-or-new visibility on filesystems with POSIX rename
semantics. It does not promise crash durability or preserve ownership, ACLs,
extended attributes, timestamps, or hardlink identity. Non-Unix platforms
currently return `Other("unsupported-save-platform")` without changing files.
Use `write_text` when creation or in-place truncation is intended. See the
[existing-file save contract](crates/capability-fs/README.md#existing-file-save)
for permission, concurrency, cleanup, and platform details.

## Choose host capabilities

| Guest import | Host implementation | Scope |
| --- | --- | --- |
| `cell:log/log@0.1.0` | `cell_capability_log::StdoutLog` | Writes guest messages to stdout. |
| `cell:fs/fs@0.1.0` | `cell_capability_fs::LocalFs::new(root)?` | Read/write inside a host-approved root. |
| `cell:fs/fs@0.1.0` | `cell_capability_fs::ReadOnlyFs::new(root)?` | Read-only access inside a host-approved root. |
| `cell:fs/fs@0.1.0` | `cell_capability_fs::Workspace` | Named roots, each with its own write policy. |
| `cell:openai/chat@0.1.0` | `cell_capability_openai::OpenAiChat` | Chat completions at a host-configured endpoint. |

Filesystem paths are relative to the configured root. The filesystem host implementation rejects absolute guest paths, parent traversal, and symlink resolution that escapes the grant. Operations use retained `cap-std` directory handles, not a canonicalize/check followed by an ordinary host-path operation; replacing the original root pathname does not retarget an existing capability.

Intermediate and final relative symlinks may be followed only when resolution remains within the grant. Absolute symlink targets are rejected even if they name a location inside the root. `write_text` creates or truncates a file, but cannot follow a symlink to an outside target. `remove_file` unlinks a final symlink itself, not its target. `symlink_metadata` and directory entry types do not follow final symlinks.

These are path-resolution guarantees, not host namespace isolation: a trusted host can move an opened directory, and preexisting hardlinks or mounts remain part of the grant. Consumers do not need a canonicalize/check/write preflight. See the [filesystem capability contract](crates/capability-fs/README.md) for operation semantics, concurrency limits, and regression tests.

For multiple roots, `Workspace` exposes mounts as top-level guest directories:

```rust
use cell::Cell;
use cell_capability_fs::Workspace;

let workspace = Workspace::new()
    .mount_read_only("project", "./project")?
    .mount("scratch", "./scratch")?;

let cell = Cell::new()?.with(workspace)?;
```

The guest can refer to `project/src/lib.rs` or `scratch/notes.txt`; writes to `project/` are denied.

For chat completions, only the `complete` function crosses the guest boundary. Model, credentials, endpoint, timeout, and outbound network requests belong to the host:

```rust
use cell::Cell;
use cell_capability_openai::OpenAiChat;

let cell = Cell::new()?
    .with(OpenAiChat::new("gpt-4o-mini", std::env::var("OPENAI_API_KEY")?)?)?;

let local_cell = Cell::new()?
    .with(OpenAiChat::local("llama3.1", "http://localhost:11434/v1")?)?;
```

Using the public endpoint sends chat content to the configured provider and requires credentials. Local endpoints require the host to run a compatible server.

### Observe filesystem changes from the host

`LocalFs`, `ReadOnlyFs`, and `Workspace` expose native Rust polling watches.
These are **host-only APIs**, not WIT imports or `cell_guest::fs` functions;
watching does not change the component ABI or grant guest access to notifications.

```rust
use cell_capability_fs::{ReadOnlyFs, WatchEvent, WatchOptions};

let filesystem = ReadOnlyFs::new("./project")?;
let mut watch = filesystem.watch(WatchOptions::default())?;
// Call periodically from your own event loop; this scans once without waiting.
for event in watch.poll() {
    match event {
        WatchEvent::Created(path) => println!("created: {path}"),
        WatchEvent::Modified(path) => println!("modified: {path}"),
        WatchEvent::Deleted(path) => println!("deleted: {path}"),
        WatchEvent::RescanRequired(reason) => {
            println!("refresh cached state through the capability: {reason:?}");
        }
        _ => {} // WatchEvent is non-exhaustive.
    }
}
watch.cancel();
```

Construction takes an initial snapshot, without creation events for existing
entries. Each `poll()` synchronously scans the whole granted tree; workspace
paths include mount names. Read-only watches can observe other writers without
write authority. Scans use retained directory handles and never traverse
symlinks, including directory symlinks inside the grant.

Choose nonzero `max_entries` and `max_events` budgets for your workload
(defaults: 100,000 entries and 10,000 events). Initial scan failure returns an
`FsError`; later scan failures or budget overflows return `RescanRequired`
instead of a partial change list. Refresh your own cached state on that event.
Renames appear as deletion plus creation. Metadata fingerprints are not content
hashes, so changes can be missed or coalesced between polls; this is not an
OS event stream, atomic snapshot, or complete history. Schedule polling yourself:
there are no background threads. Cancelling or dropping the watch releases its
retained authority; a cancelled watch returns no events.

See the [native polling watch contract](crates/capability-fs/README.md#native-polling-watches)
for rescan recovery, traversal limits, and lifecycle details.

## List installed capabilities

Host code can inspect the capabilities installed in a particular Cell with `Cell::capabilities()`:

```rust
use cell::Cell;
use cell_capability_log::StdoutLog;

let cell = Cell::new()?.with(StdoutLog)?;
assert_eq!(cell.capabilities(), &["cell:log/log@0.1.0"]);

for name in cell.capabilities() {
    println!("{name}");
}
```

The method returns a borrowed slice of unique names in installation order. An empty Cell returns an empty list, and names are recorded only after successful `.with(...)` registrations. Built-in capabilities report their versioned WIT interface names: `cell:log/log@0.1.0`, `cell:fs/fs@0.1.0`, and `cell:openai/chat@0.1.0`. All filesystem implementations use the same filesystem interface name.

This list is host-side metadata, not a guest-visible discovery interface or a catalog of all available capability crates. It does not describe filesystem roots, read/write policies, credentials, or other configuration, and listing a capability does not grant a guest access to it: the guest still needs the matching WIT import. Registrations made directly through `Cell::linker_mut()` are not tracked.

Custom capabilities can override `Capability::name()` as shown below. Without an override, the name defaults to the implementation's Rust type name, so existing capability implementations continue to work unchanged.

### Describe installed authority

Use `Cell::capability_descriptors()` when a display tool needs more than interface
identities. For example, read-only and writable filesystems both expose
`cell:fs/fs@0.1.0`, but their descriptors report different write policies and
policy-permitted operations:

```rust
use cell::{AuthorityDescription, Cell};
use cell_capability_fs::ReadOnlyFs;

let cell = Cell::new()?.with(
    ReadOnlyFs::new("./workspace")?.with_logical_name("project")
)?;
for descriptor in cell.capability_descriptors() {
    println!("{}", descriptor.interface);
    if let AuthorityDescription::Filesystem { roots } = &descriptor.authority {
        for root in roots {
            println!("{}: read={:?}, write={:?}, operations={:?}",
                root.name, root.read_policy, root.write_policy, root.operations);
        }
    }
}
```

`LocalFs` and `ReadOnlyFs` use `/` as their default logical root name; an optional
`with_logical_name(...)` label changes only display metadata, not path resolution
or authority. `Workspace` describes each named mount independently, including
its read/write policies. Descriptors do not include physical host paths.

**Capability descriptions are advisory metadata, not authorization checks.**
Operations continue to enforce their actual authority independently, and host
permissions or other runtime conditions can still cause a described operation to
fail. Descriptors are installation-time snapshots and may become stale if a
custom capability changes its policy.

Descriptors record successful `.with(...)` installations in order without
identity deduplication. Direct linker registrations and composed guest providers
are not tracked. Custom capabilities can override `Capability::descriptor()`;
the default reports their identity with `AuthorityDescription::Unspecified`,
which implies neither a grant nor a denial. The authority model is non-exhaustive
to support future non-filesystem descriptions. Descriptor producers and hosts
choosing display labels must never include credentials, secrets, or physical
host paths.

## Inspect and compose components

Use `cell-compose` from host-side code to inspect component boundaries and explicitly
wire an application to embedded provider components. For a host crate under
`examples/`, add:

```toml
[dependencies]
anyhow = "1"
cell-compose = { path = "../../crates/cell-compose" }
```

For a separate project, use the same Git URL and commit pin as `cell`; see the
[downstream dependency policy](README.md#downstream-dependencies). These are library
APIs; no CLI or manifest format is required.

| API | Purpose |
| --- | --- |
| `inspect_component(bytes)` | Inspect imports, exports, nested interface members, and function signature summaries without a runtime engine or composition plan. |
| `Composer::describe()` | Describe the intended graph, including incomplete or invalid wiring. |
| `Composer::check()` | Validate complete composition, including wiring, types, cycles, and encoding, without retaining the output bytes. |
| `Composer::compose()` | Perform the same validation and produce the composed component. |
| `Cell::check(&component)` | Validate a compiled component's imports against the actual linker without instantiating it. |
| `Composer::check_with(&cell)` | Validate composition, compile it, and check remaining host imports against the Cell's linker without guest execution. |

### Inspect a component boundary

```rust
use cell_compose::inspect_component;

fn main() -> anyhow::Result<()> {
    let bytes = std::fs::read("application.wasm")?;
    let description = inspect_component(&bytes)?;
    for import in &description.imports {
        println!("import {}: {:?}", import.name, import.kind);
    }
    for export in &description.exports {
        println!("export {}: {:?}", export.name, export.kind);
    }
    Ok(())
}
```

Inputs must be valid binary WebAssembly components, not core modules or WAT.
Descriptions expose names and kinds (`Function`, `Interface`, `Module`,
`Component`, `Type`, or `Value`). Each `ItemDescription` also has recursively expanded,
lexically sorted interface `members` and an optional `function` summary with named
parameters in declaration order, a result type, and async status. Primitive types
are precise; compound and resource types are summarized by category. These summaries
are for display, not structural compatibility decisions: use the check APIs for that.
`Composer::describe()` uses the same enriched component inventories.

Tiny migration guide: existing field reads are unchanged. If constructing
`ItemDescription` literals, supply `members` and `function` (`Vec::new()` and `None`
when absent); exhaustive destructuring patterns can add `..`.

### Describe and validate an explicit composition

The following assumes the application imports `example:service/api@1.0.0` and the
provider exports a compatible interface under the same name:

```rust
use cell_compose::{APPLICATION, Composer};

fn main() -> anyhow::Result<()> {
    let app = std::fs::read("application.wasm")?;
    let provider = std::fs::read("provider.wasm")?;
    let incomplete = Composer::application(app).provider("service", provider)?;

    // Missing wiring is visible rather than preventing description.
    let graph = incomplete.describe()?;
    for import in &graph.unresolved_imports {
        println!(
            "{}.{}: {:?}, host declared: {}",
            import.endpoint.component,
            import.endpoint.item,
            import.kind,
            import.host_declared,
        );
    }
    assert!(incomplete.check().is_err());

    let plan = incomplete.connect(
        APPLICATION,
        "example:service/api@1.0.0",
        "service",
        "example:service/api@1.0.0",
    );
    plan.check()?;
    let bytes = plan.compose()?;
    std::fs::write("composed.wasm", bytes)?;
    Ok(())
}
```

Provider names such as `service` are plan-local identifiers. Connections use exact
top-level import and export names; there is no automatic name-based wiring. All
supplied providers are included, and their imports also require explicit wiring
or host declarations. Only the application's exports become output exports.
Calling `check()` before `compose()` is optional: `compose()` already performs the
same complete validation.

`GraphDescription` contains component inventories and roles, declared connections,
host declarations, unresolved imports, and output-export endpoints. Descriptions
are owned data with deterministic ordering: the application comes first, providers
are sorted by name, and other inventories are sorted lexically by name or endpoint.
They contain no WAC-internal IDs. A missing application or invalid component binary
still makes `describe()` fail, but missing connections do not. Duplicate connections
and stale or invalid host declarations remain visible for validation to reject.
An import with a declared connection is not listed as unresolved, even if that
connection is invalid; use `check()` to establish validity.

### Distinguish host requirements from installed capabilities

To intentionally leave an interface import for the host, declare it on its consumer:

```rust
let plan = plan.allow_host_import("service", "cell:log/log@0.1.0");
```

This assumes the `service` provider actually imports that interface and it has no
connection for it. Unconnected imports remain in `unresolved_imports`:

- `host_declared: true` means an explicit host declaration exists, not that a host
  implementation is installed. Validation still checks that it is an interface.
- `host_declared: false` means the dependency is missing wiring and host permission;
  `check()` and `compose()` reject it.

Composition validity does **not** prove runtime host satisfiability. Install the
matching implementations in the `Cell` before instantiating the composed component.
Host declarations grant no capabilities, and `Cell::capabilities()` is separate
host-side registration metadata, not composition validation. Use `plan.check_with(&cell)?`
to validate the wiring and the actual linker requirements without instantiating either
the application or its providers. `plan.compile(&cell)?` alone checks compilation,
not linker satisfiability. For an already-compiled component, use `cell.check(&component)?`.

```rust
let cell = cell::Cell::new()?.with(cell_capability_log::StdoutLog)?;
plan.check_with(&cell)?; // Composition and host-interface checks; no store needed.
let component = plan.compile(&cell)?;
cell.check(&component)?; // Optional here; useful for standalone components too.
```

This example assumes logging is the plan's only remaining host requirement; the host
must depend on `cell-capability-log` to install `StdoutLog`. Preflight checks the actual
linker, including direct registrations, rather than relying on tracked capability names.
Missing implementations and incompatible interface members produce linking diagnostics.
No guest start functions or host callbacks run during these checks.

Successful preflight establishes static link compatibility only. Startup can still
trap, execution can exceed resource limits, and installed capabilities can deny
individual operations under their runtime policies. Inspection, compilation, and
link validation are separate from instantiation and execution.

Inspecting an already-composed binary reveals its external imports and exports,
not its original provider names or connection provenance. Recovering that original
graph would require metadata that Cell does not currently embed. See the
[`cell-compose` documentation](crates/cell-compose/README.md) for composition rules
and runtime limitations, and [`examples/compose-demo`](examples/compose-demo) for a
working provider/application example.

## Add an independent capability

A capability crate can live outside this repository. It owns its WIT contract, host implementation, and Cell registration code. Cell needs no central registry entry.

A minimal capability package usually contains:

```text
capability-example/
├── Cargo.toml
├── wit/
│   └── example.wit
└── src/
    └── lib.rs
```

Its WIT defines the guest-visible interface and a host world for binding generation:

```wit
package example:service@0.1.0;

interface service {
    call: func(input: string) -> result<string, string>;
}

world host {
    import service;
}
```

The capability implementation registers the matching interface with Wasmtime by implementing `cell::Capability<T>`:

```rust
use wasmtime::component::Linker;

pub struct ExampleService;

impl<T: 'static> cell::Capability<T> for ExampleService {
    fn name(&self) -> &'static str {
        "example:service/service@0.1.0"
    }

    fn add_to_linker(self, linker: &mut Linker<T>) -> anyhow::Result<()> {
        linker.instance("example:service/service@0.1.0")?.func_wrap(
            "call",
            |_store, (input,): (String,)| Ok((Ok::<String, String>(input),)),
        )?;
        Ok(())
    }
}
```

Generated bindings are often preferable for real capabilities; see [`crates/capability-log`](crates/capability-log), [`crates/capability-fs`](crates/capability-fs), and [`crates/capability-openai`](crates/capability-openai) for working patterns.

## Consumer checklist

- [ ] Choose a reproducible dependency source; for the local starter, keep the generated lockfile and record the Cell revision and local changes.
- [ ] Define the guest's WIT world.
- [ ] Import only the capabilities the guest actually needs.
- [ ] Include matching WIT dependency packages under `wit/deps/` or equivalent, and keep their definitions synchronized with the host capability and guest helper versions. The standalone starter stages these automatically; rerun its bootstrap rather than editing generated WIT.
- [ ] For non-following filesystem traversal, inspect metadata or directory entry types and skip symlinks; propagate lookup failures rather than treating them as entry types.
- [ ] For existing-file edits, use revisioned reads and saves, handle conflicts explicitly, and arrange external synchronization if lost updates are unacceptable.
- [ ] Generate guest bindings with `wit-bindgen`.
- [ ] In guest code, call the generated/Cell guest bindings for capabilities instead of host-based APIs such as Rust's `std::fs`.
- [ ] Set the guest crate type to `cdylib`.
- [ ] Build the guest for the appropriate Wasm target.
- [ ] Inspect the resulting component's imports and exports with `cell_compose::inspect_component` when checking its boundary.
- [ ] If composing providers, use `Composer::describe()` to review the graph, explicitly connect dependencies or declare host interfaces, and validate with `check()` or `compose()`.
- [ ] Install matching host capability implementations with `Cell::new()?.with(...)`.
- [ ] Test both success and missing-capability failure cases.

## Capability design checklist

Good capability requests and implementations usually:

- Keep the WIT interface narrow.
- Prefer explicit inputs and outputs over hidden global state.
- Avoid broad ambient authority.
- Treat the WIT package name and version as public API.
- Override `Capability::name()` with a meaningful public identifier, such as the versioned WIT interface name, for host-side capability listings.
- Keep generated bindings and provider-specific details inside the capability crate when possible.
- Explain what happens when the capability is missing or denied.

## Handle filesystem errors by category

Guest filesystem functions return `cell_guest::fs::FsError`, not strings.
Use `kind`, `path`, and `operation` rather than parsing `Display` output:

```rust,ignore
use cell_guest::{CellPath, fs::{self, FsError, FsErrorKind}};

fn read_optional(path: &CellPath) -> Result<Option<alloc::string::String>, FsError> {
    match fs::read_text(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) => match &error.kind {
            FsErrorKind::NotFound => Ok(None),
            FsErrorKind::PermissionDenied | FsErrorKind::ConfinementViolation => Err(error),
            _ => Err(error), // Includes future/Other categories.
        },
    }
}
```

`FsErrorKind` and `FsError` are non-exhaustive Rust types. The WIT error record
contains an extensible category (`other(string)` for unknown categories), the
original logical path, and the operation; it never contains raw host filesystem
messages or resolved targets. `exists` still returns only a boolean.
If an application's own exported WIT function returns a string error, convert
explicitly at that application boundary with `.map_err(|error| error.to_string())`,
as `fs-demo` does. Keep category matching inside the application where possible.

This is a breaking change from the old `result<_, string>` filesystem interface:
rebuild guests and hosts together and synchronize downstream filesystem WIT
copies. See [filesystem error semantics](crates/capability-fs/README.md#typed-errors-and-compatibility)
for the full category contract, backend-classification caveat, and confinement
limits.
