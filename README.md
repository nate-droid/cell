# Cell

Cell is a small Rust runtime for running WebAssembly components with [Wasmtime](https://wasmtime.dev/) and the WebAssembly Component Model. A `Cell` starts with **no host capabilities**: it does not install WASI, filesystem, network, clocks, environment, or logging for a guest. The host opts into each imported interface by installing a capability.

Downstream guests should use Cell capabilities, not ambient host APIs. For example, a Rust guest that needs filesystem access should import `cell:fs/fs@0.1.0` and use the guest-side Cell filesystem bindings/helpers rather than calling `std::fs`, which depends on WASI-style host filesystem authority that Cell does not install by default.

Cell is a library for Rust host applications and WIT-based guests, not a standalone CLI or a WASI replacement. Rust APIs and WIT contracts are experimental. Start with the logging example below, then try the read-only filesystem starter.

## Try it

From the repository root, use a current stable Rust toolchain with Cargo, a native linker, and `make`. This checkout was validated with Rust/Cargo 1.97.1 on Apple Silicon macOS; a minimum supported Rust version and cross-platform release matrix are not yet established. Install the guest target:

```sh
rustup target add wasm32-unknown-unknown
```

Then run (Cargo may download third-party dependencies on the first build):

```sh
make hello
```

This builds the guest and runs the checked-in `hello-host` application. Expected output:

```text
hello from a Cell guest
```

To build only the guest, run `cargo build -p hello --target wasm32-unknown-unknown`. The example host encodes this core module as a component using its embedded WIT metadata; no WASI interfaces are installed.

The `hello` guest imports `cell:log/log@0.1.0` and exports `run`. A host application can run the built component with:

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

Paths above are relative to the repository root. The `std::fs::read` call is host-side code that loads the compiled component bytes; it does not grant the guest filesystem access. A host needs `cell`, `cell-capability-log`, `anyhow`, and `wit-component = "0.241.2"` as dependencies; see [Downstream dependencies](#downstream-dependencies) for the external source policy. The guest's WIT imports must match the capabilities the host installs; without `StdoutLog`, this guest fails to instantiate.

For a copyable, standalone host-and-guest workspace, see the [downstream starter](examples/downstream-starter/README.md). It automatically stages package inputs and matching WIT from a local Cell checkout, builds a guest, and runs it with logging and read-only filesystem authority. This is an interim local workflow, not a published release. Run `make downstream-starter` to validate it with cached dependencies.

### Read-only filesystem and denied authority

With Python 3.11+ and the prerequisites above, run from the repository root:

```sh
python3 examples/downstream-starter/run.py .
```

Expected application output (in addition to Cargo build messages):

```text
downstream guest started
read=hello from the downstream fixture; write=permission-denied
```

This stages Cell's package inputs and matching WIT into a standalone workspace, builds the guest, and runs it with **only logging and a rooted `ReadOnlyFs` grant**. The host creates `input.txt`; the guest reads it and confirms that creating `denied.txt` is refused. No WASI or general network interface is installed. Append `--offline` only when registry dependencies are cached. See the [starter guide](examples/downstream-starter/README.md) to copy it outside this checkout or use your own root.

Absent interfaces and denied operations are different: omitting filesystem registration prevents this importing guest from instantiating; installing `ReadOnlyFs` satisfies the import but rejects mutations at runtime. Observe both cases without editing code:

```sh
cargo test -p cell --test fs_demo -- --nocapture
```

The three tests cover missing filesystem authority, successful read/write with `LocalFs`, and write denial with `ReadOnlyFs`.

For setup instructions, other capabilities, and guest development, see [Building Projects with Cell](BUILDING_WITH_CELL.md).

For the experimental open-source release roadmap and acceptance criteria, see the [Release Checklist](RELEASE_CHECKLIST.md).

## Static inspection and preflight

`cell_compose::inspect_component(bytes)` validates a binary and describes its imports,
exports, nested interface members, and function signature summaries without an engine
or guest execution. `Composer::describe()` uses the same inventory and still accepts
incomplete or invalid wiring so tools can explain unresolved dependencies.

- `Composer::check()` validates explicit bindings, structural interface compatibility,
  unresolved dependencies, and cycles through composition encoding. Allowed host
  imports are declarations, not installed capabilities.
- `Cell::check(&component)` checks all remaining imports against the actual linker,
  including direct `linker_mut()` registrations, without a store or instantiation.
  Errors preserve Wasmtime's detailed member/type diagnostics and Cell's capability guidance.
- `Composer::check_with(&cell)` combines composition validation with runtime-linker
  preflight, compiling but never instantiating the application or its providers.

A successful preflight establishes static link compatibility, not that execution will
succeed: startup traps, execution limits, and operation-specific capability policies
remain runtime concerns. These APIs do not call guest start functions or host callbacks.
Existing compilation and execution APIs retain their behavior.

```rust
let description = composer.describe()?; // May contain unresolved imports.
composer.check_with(&cell)?;             // No guest execution or host state needed.
let component = composer.compile(&cell)?;
cell.check(&component)?;                 // Also usable for standalone components.
```

Inspection migration: `ItemDescription` now includes `members` and `function`.
Callers constructing struct literals must supply these fields (use `Vec::new()` and
`None` when absent); exhaustive patterns can add `..`. Function summaries are for
display, not type-compatibility decisions; compound types are summarized by category.

## Content-addressed component artifacts

`LocalArtifactStore` imports binary WASM components from any toolchain into a
caller-selected local directory. Identity is SHA-256 over the exact source bytes
(`sha256:<64 lowercase hex digits>`), not over capabilities, permissions, or
execution policy. Identical bytes reuse the same entry across executions.

```rust,no_run
use cell::{ArtifactDigest, Cell, LocalArtifactStore};

# fn main() -> anyhow::Result<()> {
let artifacts = LocalArtifactStore::new(".cell/artifacts");
let digest = artifacts.put_file("component.wasm")?;
println!("{digest}"); // Persist this identifier for later executions.
let saved: ArtifactDigest = digest.to_string().parse()?;
let cell = Cell::new()?;
let component = cell.component_from_artifact(&artifacts, saved)?;
let instance = cell.instantiate(&mut cell.store(()), &component)?;
# Ok(())
# }
```

Publication is atomic and does not overwrite existing entries. Resolution verifies
integrity before loading; Cell compiles those same verified bytes rather than
reopening a path. Missing or corrupted artifacts return errors. Import accepts
binary component headers only (not core modules or WAT); full component validation
happens during loading. The store contains portable source bytes, not native
serialized Wasmtime code, and has no registry, dependency resolution, or policy
metadata. A digest proves byte integrity, not publisher trust: obtain expected
identifiers through a trusted channel when authenticity matters.

**Compatibility:** Existing `component_from_bytes` and instantiation APIs are
unchanged; storage is opt-in and requires no migration. Install capabilities and
choose execution limits separately as before.

## Execution budgets and resource limits

Use the managed API for untrusted execution. Existing `store`, `instantiate`, and raw Wasmtime calls remain available for compatibility, but do not enforce an `ExecutionPolicy`.

```rust
use cell::{CancellationToken, Cell, ExecutionPolicy};
use std::time::{Duration, Instant};

let cell = Cell::new()?;
let component = cell.component_from_bytes("(component)")?;
let cancellation = CancellationToken::default();
let policy = ExecutionPolicy {
    fuel: Some(1_000_000),
    memory_bytes: Some(16 * 1024 * 1024),
    deadline: Some(Instant::now() + Duration::from_secs(2)),
    cancellation: Some(cancellation.clone()),
    input_bytes: Some(64 * 1024),
    output_bytes: Some(64 * 1024),
};
let mut store = cell.managed_store((), policy);
let instance = cell.instantiate_managed(&mut store, &component)?;
// For a component exporting `run`:
// let results = store.call(&instance, "run", &[])?;
// Another thread can call cancellation.cancel().
```

`ExecutionPolicy::default()` imposes no Cell-configured limits. Fuel is refreshed for each instantiation and invocation; canonical post-return shares the invocation budget. Startup functions run under the same controls. Deadlines are absolute (including time between calls), and cancellation tokens are sticky. Cancellation and deadlines are checked at operation boundaries and guest epoch safepoints, with a nominal 1 ms ticker, not a guaranteed response time. Concurrent stores on one engine check their own policies independently.

`ExecutionError.kind` distinguishes `Trap`, `Fuel`, `Memory`, `Deadline`, `Cancelled`, `Invalid`, `Input`, and `Output`; the original error remains available through `std::error::Error::source`. After a guest trap, discard the managed store and instantiate in a fresh one: canonical ABI state may be poisoned. The same `Cell` and compiled component can be reused.

### Enforcement boundaries

- Memory limits are **per guest linear memory**, including allocation at startup and growth, not an aggregate bound across multiple memories or a process RSS limit. Tables and native host allocations are not covered.
- Input/output limits count recursive logical component values, with at least one byte per node plus string bytes and scalar widths. They are not serialized wire sizes. Inputs are checked before calling the guest; outputs are checked after Wasmtime lifts them, so an output limit does not prevent the initial host allocation.
- Managed calls do not expose mutable raw stores. Guest code, including composed providers and guest code invoked by hosts in the same store, remains subject to store fuel, memory, and epoch controls. Fuel does not charge native host CPU work.
- **Synchronous native host operations cannot be forcibly interrupted.** Wasmtime 47 component calls do not dispatch store call hooks. A blocking filesystem/network/custom host call can delay cancellation or a deadline until control returns to guest code or the managed operation returns. Host implementations must bound their own requests/responses and allocations, use I/O timeouts, and cooperate with cancellation. Application payload limits do not automatically bound arbitrary capability-call arguments or results.
- Component compilation and linking, host effects already performed, and malicious host implementations are outside guest execution isolation. For strict wall-clock or total-memory containment, run the runtime in a separately resource-limited worker process.

### Tiny migration guide

No source migration is required for callers using `cell.store(...)`. To opt into enforcement, replace it with `cell.managed_store(state, policy)`, use `instantiate_managed`, and replace typed/raw calls with `store.call(&instance, export_name, &[wasmtime::component::Val::...])`. Managed calls handle canonical post-return automatically. Host state remains `T`, so capability implementations need no binding changes.

Cell engines now enable fuel and epoch instrumentation. If you construct `wasmtime::Store::new(cell.engine(), state)` directly, initialize its fuel and epoch deadline yourself, or switch to `cell.store(state)`; a raw newly constructed Wasmtime store otherwise starts with zero fuel and an expired epoch deadline.

## Downstream dependencies

The public crates currently share workspace version **0.1.0**. Their APIs and WIT contracts are **experimental**; no stability guarantee or published release is implied by that version.

| Classification | Public crates / contracts |
| --- | --- |
| Host runtime API | `cell` (`Cell`, `Capability`); `cell-compose` (optional component composition) |
| Host capability APIs | `cell-capability-fs` (`LocalFs`, `ReadOnlyFs`, `Workspace`), `cell-capability-log`, `cell-capability-openai` |
| Guest API | `cell-guest` (`CellPath`, `CellPathBuf`, `fs` helpers) |
| Interface/bindings support | The `wit/` trees shipped inside the capability and guest crates; filesystem contract `cell:fs/fs@0.1.0` |
| Internal implementation details | Private binding modules, confinement and transport implementation, composition graph internals; examples are not dependency APIs |

There is no separate public bindings crate. The filesystem host and guest crates generate their bindings from package-local WIT. Guests defining their own world can copy the filesystem interface from `crates/guest/wit/deps/cell-fs/fs.wit` into their world's WIT dependency tree. Keep that interface aligned with the pinned Cell revision; do not reference a sibling Cell checkout from binding generation. Host and guest Rust binding types are generated separately.

**Recommended interim source: Git, pinned to a full commit SHA.** This checkout currently has **no configured Git remote or release tags**, so there is not yet a canonical URL to put in a downstream manifest. Once maintainers designate the canonical repository, use the following syntax (the capitalized values are inputs to replace, not existing release coordinates):

```toml
# Host project
[dependencies]
cell = { git = "CANONICAL_REPOSITORY_URL", rev = "FULL_COMMIT_SHA" }
cell-capability-fs = { git = "CANONICAL_REPOSITORY_URL", rev = "FULL_COMMIT_SHA" }
# Optional: cell-compose, cell-capability-log, cell-capability-openai
# use the same git URL and rev.
```

```toml
# Separate guest project
[dependencies]
cell-guest = { git = "CANONICAL_REPOSITORY_URL", rev = "FULL_COMMIT_SHA" }
```

Cargo locates these packages in the Git workspace and resolves their internal path dependencies within its own checkout. Consumers need neither `[workspace.dependencies]` inheritance nor a `../cell` directory. Pin every Cell crate to the same source/revision and commit the application's `Cargo.lock`. A future release-tag policy can replace commit pinning when maintainers establish one; none exists today.

**Registry consumption is not currently configured or verified.** The manifest version `0.1.0` is not evidence that these packages are available on crates.io. Internal library dependencies now carry both that version and a local path, preparing for registry distribution without breaking workspace development. Publication still requires matching Cargo license metadata for the existing MIT license, canonical repository metadata, confirmed package availability/ownership, and a release process. Do not use bare registry dependencies until releases are actually available.

Library WIT/build inputs are crate-local and included in Cargo's package file lists. The core filesystem integration tests intentionally remain Git-checkout tests: they build the repository's `fs-demo` example and require `wasm32-unknown-unknown`. They are not standalone archive tests.

### Verify the downstream boundary

Run `make external-consumer` (Python 3.11+, Cargo, and cached Rust dependencies required). The standalone fixture at `tests/external-consumer` declares all six public crates explicitly. Its script:

- obtains each crate's distributable inputs with `cargo package --list --allow-dirty --offline`, including WIT;
- copies only those inputs to a temporary directory outside the repository;
- resolves inherited manifest values into explicit standalone values and substitutes staged packages for unpublished registry dependencies;
- checks a non-workspace consumer importing runtime, filesystem host/guest APIs, generated filesystem types, and the optional public crates.

The fixture's registry-looking versions are **test inputs**, patched to isolated package directories by the script, not advertised release coordinates. No dependency resolves back into the original source tree. This tests the library package boundary, not network Git fetching or registry publication. Once a canonical remote exists, additionally validate a fresh consumer using the pinned Git form above.

## How it fits together

| Piece | Responsibility |
| --- | --- |
| Guest component | Declares its imports and exports in a WIT world; it receives only matching host interfaces. |
| Capability crate | Owns a WIT interface and registers a host implementation with Cell. |
| Host application | Constructs a `Cell`, installs capabilities with `.with(...)`, and runs a component. |

The core crate, [`crates/cell`](crates/cell), only manages the Wasmtime engine, linker, store, component compilation, and instantiation. Capabilities can live in independent crates: implementing `cell::Capability<T>` is enough to install one with `.with(...)`. No central capability registry or fixed WIT world is required.

```rust
let cell = Cell::new()?
    .with(StdoutLog)?
    .with(ReadOnlyFs::new("./workspace")?)?;
```

This grants the guest access to logging and a rooted read-only filesystem **only if its WIT world imports those interfaces**. A capability not installed by the host cannot be used by the guest. `Cell::<MyState>::empty()?` creates a Cell with a custom store-state type; the default `Cell::new()?` uses `()`.

Filesystem grants use retained directory handles and confined symlink resolution, not lexical checks followed by ambient path operations. Guests can inspect entries with `cell_guest::fs::symlink_metadata` and recurse through `list_dir` entries while skipping symlinks. See the [filesystem capability contract](crates/capability-fs/README.md) for symlink semantics, race guarantees, and grant limits.

## Load capabilities from WASM components

[`cell-compose`](crates/cell-compose/README.md) adds explicitly wired component providers alongside native capabilities. An application imports a WIT interface; a separately built provider exports that interface. Cell compiles and runs the composed component without requiring a native Rust implementation of that interface.

```rust
use cell::Cell;
use cell_compose::{Composer, APPLICATION};

let app = std::fs::read("formatter-app.wasm")?;
let provider = std::fs::read("formatter-provider.wasm")?;
let cell = Cell::new()?;
let component = Composer::application(app)
    .provider("formatter", provider)?
    .connect(
        APPLICATION,
        "cell-example:formatter/formatter@0.1.0",
        "formatter",
        "cell-example:formatter/formatter@0.1.0",
    )
    .compile(&cell)?;
let mut store = cell.store(());
let instance = cell.instantiate(&mut store, &component)?;
let run = instance.get_typed_func::<(String,), (String,)>(&mut store, "run")?;
let (output,) = run.call(&mut store, ("hello".to_owned(),))?;

assert_eq!(output, "formatted: hello");
```

See the [formatter demo](examples/compose-demo/README.md) for WIT contracts and complete build/run instructions. Component inputs must be binary components, not core WASM modules.

Every provider dependency must be connected explicitly or declared with `.allow_host_import(consumer, interface)`. That declaration installs nothing: any remaining host interface must still be supplied through native capabilities. Missing dependencies, incompatible connections, and cycles fail composition. Only the application's exports are exposed.

Each provider is instantiated once per composed instance. Consumers of the same provider share its state; separate composed instances have separate guest state. Components in the graph share a store-level budget, not independent isolation or quotas. Same-name remaining host interfaces may be merged, so this API does not provide distinct native authority per consumer. `Cell::capabilities()` continues to list native registrations only.

## List installed capabilities

Use `Cell::capabilities()` to inspect the capabilities installed in a particular Cell:

```rust
let cell = Cell::new()?.with(StdoutLog)?;
assert_eq!(cell.capabilities(), &["cell:log/log@0.1.0"]);
for name in cell.capabilities() {
    println!("{name}");
}
```

An empty Cell returns an empty list. Names are unique and appear in installation order after successful `.with(...)` calls. Built-in capabilities use their versioned WIT interface names; custom capabilities can override `Capability::name()`, which defaults to the Rust type name.

This is host-side metadata, not guest-side discovery or a catalog of every capability crate. It does not expose credentials, filesystem roots, or permission policies. Registrations made directly through `linker_mut()` are not tracked.

### Structured authority descriptions

`Cell::capability_descriptors()` returns structured, safe-to-display authority
snapshots for successful `.with(...)` installations. `Capability::descriptor()`
defaults to identity plus `AuthorityDescription::Unspecified`, so existing custom
capabilities remain compatible. The non-exhaustive authority model can grow to
include non-filesystem kinds. Descriptors retain installation order and are not
deduplicated; `capabilities()` continues to return unique interface identities.

Filesystem descriptions contain the interface identity and a list of logical
roots, each with read/write policies and policy-permitted operations. `LocalFs`
and `ReadOnlyFs` default to the logical name `/`; hosts can set a display label
with `.with_logical_name("project")`. `Workspace` reports each named mount and
its independent policy. Physical host paths are never inferred or included.
Custom descriptor producers and hosts choosing display labels must not include
credentials, secrets, or physical host paths.

```rust
let cell = Cell::new()?.with(
    ReadOnlyFs::new("./workspace")?.with_logical_name("project")
)?;
for descriptor in cell.capability_descriptors() {
    println!("{descriptor:?}");
}
```

**Descriptions are advisory metadata, not authorization checks.** They do not
permit operations or guarantee success: runtime confinement, configured write
permissions, and host filesystem permissions are enforced independently on each
operation. Mutable capabilities may change policy after the snapshot is taken.
Direct `linker_mut()` registrations and composed guest providers are not tracked.

## Workspace map

- [`crates/cell`](crates/cell): capability-neutral host runtime.
- [`crates/cell-compose`](crates/cell-compose): explicit component-provider composition using WIT-compatible imports and exports.
- [`crates/capability-log`](crates/capability-log): `cell:log/log@0.1.0` and `StdoutLog`.
- [`crates/capability-fs`](crates/capability-fs): `cell:fs/fs@0.1.0`; `LocalFs`, `ReadOnlyFs`, and multi-root `Workspace`.
- [`crates/capability-openai`](crates/capability-openai): `cell:openai/chat@0.1.0`; `OpenAiChat` for an OpenAI-compatible chat-completions endpoint. Credentials and network access remain host-side.
- [`crates/guest`](crates/guest): guest-side `cell_guest::CellPath` and filesystem helpers (not a host capability).
- [`examples/compose-demo`](examples/compose-demo): host runner composing the WIT-defined formatter provider and application.
- [`examples/formatter-provider`](examples/formatter-provider): pure WASM formatter capability.
- [`examples/formatter-app`](examples/formatter-app): application importing the formatter interface.
- [`examples/hello`](examples/hello): minimal logging guest.
- [`examples/hello-host`](examples/hello-host): runnable host granting only logging; run with `make hello`.
- [`examples/fs-demo`](examples/fs-demo): filesystem guest used by the core integration tests.

## Design boundaries

- **No ambient authority:** no host interface is added merely because Wasmtime or a capability crate supports it. Guests must declare imports, and hosts must install matching implementations. In Rust guests, prefer Cell capability bindings such as `cell_guest::fs` over host-based APIs such as `std::fs`.
- **Small core, open extension:** capability-specific bindings and policies belong in their own crates, including third-party crates and application-defined WIT namespaces.
- **Least privilege:** choose the narrowest implementation and root needed (for example, `ReadOnlyFs` rather than `LocalFs`).

Managed fuel, per-linear-memory limits, deadlines, cancellation, and logical input/output limits are implemented; see [enforcement boundaries](#enforcement-boundaries). Inter-Cell communication, isolated per-provider host bindings, and independent provider quotas remain future work. Component composition is opt-in through `cell-compose`, not part of the core capability registry. Artifact storage and inspection are library APIs, not a registry service or an inspection CLI.

## Tests, troubleshooting, and contributing

```sh
cargo build -p cell -p hello-host --locked
make smoke
cargo test --workspace --offline
cargo fmt --all -- --check
```

`make smoke` runs hello and the core runtime tests, including filesystem grant/denial tests; it builds the required guest fixtures. The workspace suite also covers capability implementations, guest helpers, and composition. Offline commands require cached dependencies. `make downstream-starter` validates a fresh isolated host/guest workspace; `make external-consumer` checks all six public crate package boundaries. See [preview readiness](docs/DEVELOPER_PREVIEW_READINESS.md) for current failures and validation limits; not every release gate passes yet.

- **Missing target:** install `wasm32-unknown-unknown` for your active toolchain. The quickstart needs neither `cargo-component` nor `wasm-tools`; explicit component-file commands in the builder guide need `wasm-tools`.
- **Missing import/link error:** compare the guest's versioned WIT imports with the host's `.with(...)` registrations. `cell.check(&component)` checks static linker compatibility without guest execution; it does not grant authority.
- **Permission denied:** a read-only grant intentionally rejects writes; check the configured root and host permissions for reads. Do not fix a denied write by granting broader authority unless the application needs it.
- **Sandboxed tests:** the filesystem suite creates Unix sockets; a terminal sandbox can reject this with `Operation not permitted`. Rerun in a suitable environment rather than treating the suite as passed.
- **Core module versus component:** `hello-host` and the starter encode embedded WIT metadata; `Cell::component_from_bytes` itself expects a component, not a bare core module.

For a first contribution, follow the workspace map, add a focused regression test alongside the affected crate, and run the relevant tests plus formatting. Capability authors should start with the [builder guide's design checklist](BUILDING_WITH_CELL.md#capability-design-checklist) and package-local WIT. Include toolchain, platform, exact command, and the full error chain in bug reports. Public contribution and private security-reporting processes remain release blockers, not established support promises.

## License

[MIT](LICENSE). Public crate Cargo license metadata still needs to be aligned before publication.
