# Cell

**A small WebAssembly component runtime that gives guests only the capabilities you choose.**

Cell is a Rust library built on [Wasmtime](https://wasmtime.dev/) and the [WebAssembly Component Model](https://component-model.bytecodealliance.org/). It lets applications run WebAssembly components, connect them through typed interfaces, and explicitly decide what those components can access.

A new Cell starts with **no host capabilities**. No filesystem, network, clocks, environment, logging, or WASI interfaces are installed automatically. If a guest needs something, its WIT imports must declare it and the host must supply a matching implementation.

This makes Cell useful for experimenting with **composable tools, language runtimes, agent harnesses, and other systems where execution authority should be explicit**.

I've been using Cell as a base for another project I've been building [Scope](https://github.com/nate-droid/scope). In addition to being really fun, it's been good validation to see how Cell can be used (and to make sure I don't break anything).

## See it in action

A guest imports Cell's logging interface and exports a `run` function. The host explicitly installs logging before running it.

```rust
use cell::Cell;
use cell_capability_log::StdoutLog;

let cell = Cell::new()?.with(StdoutLog)?;
```

That's a grant, not a global setting. A different Cell without `StdoutLog` cannot instantiate a guest that requires that logging import.

Try the complete example from the repository root:

```sh
rustup target add wasm32-unknown-unknown
make hello
```

Expected output:

```text
hello from a Cell guest
```

You'll need a recent stable Rust toolchain, Cargo, a native linker, and `make`. This checkout has been validated on Apple Silicon macOS with Rust/Cargo 1.97.1; a minimum supported Rust version and cross-platform support matrix have not yet been established. Cargo may download dependencies on first use.

The demo builds a Rust guest for `wasm32-unknown-unknown`, encodes it as a component using embedded WIT metadata, and runs it in a host that grants logging only. **No WASI interface is installed.** See [`hello-host`](examples/hello-host) for the full host code.

## What can you build with Cell?

Cell provides a handful of building blocks rather than a large framework:

| Building block | What it enables |
| --- | --- |
| **Explicit capabilities** | Grant a guest logging, confined filesystem operations, or access to an OpenAI-compatible chat endpoint through versioned WIT interfaces. |
| **Component composition** | Connect an application to other WASM components that implement its imported interfaces. |
| **Inspection and preflight** | Inspect imports and exports, and check wiring and installed host imports before running guest code. |
| **Managed execution** | Apply fuel, per-memory limits, deadlines, cancellation, and logical input/output bounds to guest execution. |
| **Component artifacts** | Store binary components by SHA-256 digest and verify their bytes when loading them again. |

These features are **available today**, but the Rust APIs and WIT contracts are experimental. The presence of a capability description never grants permission; runtime authority still comes from the host's actual registrations.

## A more interesting example: read, but don't write

Cell's filesystem capability can be rooted to a host-selected directory and limited to read-only operations.

```rust
use cell::Cell;
use cell_capability_fs::ReadOnlyFs;
use cell_capability_log::StdoutLog;

let cell = Cell::new()?
    .with(StdoutLog)?
    .with(ReadOnlyFs::new("./workspace")?)?;
```

A guest that imports the matching filesystem interface can read within the allowed root, but a write attempt is denied. To see that behavior in a complete host-and-guest example, run:

```sh
python3 examples/downstream-starter/run.py .
```

Expected application output (alongside build messages):

```text
downstream guest started
read=hello from the downstream fixture; write=permission-denied
```

This example needs Python 3.11+ as well as the build prerequisites above. The script stages a standalone workspace, creates the input fixture, and runs the guest with logging and rooted read-only filesystem access.

There is an important distinction:

- **No capability installed:** a guest requiring that interface cannot instantiate.
- **Read-only capability installed:** the interface is available, but disallowed operations fail when invoked.

Explore the [downstream starter](examples/downstream-starter/README.md) or read [Building Projects with Cell](BUILDING_WITH_CELL.md) to make your own guest and host.

## How Cell fits together

```text
            Rust host application
                     |
          Configure a Cell
          and grant capabilities
                     |
                Cell / Wasmtime
                     |
          WebAssembly component
           (WIT imports/exports)
                     |
      Optional composed WASM providers
```

There are three main pieces:

1. **The guest** declares its imports and exports in WIT. It does not automatically inherit the host's facilities.
2. **Capability implementations** provide individual interfaces, such as `cell:log/log@0.1.0` or `cell:fs/fs@0.1.0`.
3. **The host** chooses which implementations to install and how the guest is executed.

Capabilities live in separate Rust crates. Hosts can provide their own implementations; Cell doesn't require a central capability registry or a fixed WIT world.

### Composing components

A capability doesn't have to be implemented in native Rust. With [`cell-compose`](crates/cell-compose/README.md), one WASM component can export an interface that another imports.

```text
Application --imports formatter--> Formatter component
```

Connections are explicit and checked for compatibility before execution. Remaining host imports still require host-installed capabilities; composition declarations don't grant them automatically.

Try the [formatter composition demo](examples/compose-demo/README.md) for a complete example. Composed guests share a store-level execution budget; composition does not give each provider independent quotas or distinct native authority.

## Execution limits and their boundaries

For running potentially untrusted guest code, Cell provides **managed execution** with an explicit `ExecutionPolicy`. It supports fuel, guest linear-memory limits, deadlines, cooperative cancellation, and logical input/output bounds.

These are useful controls, **not a guarantee of total process isolation**. In particular:

- Memory limits apply per guest linear memory, not to all host memory or process RSS.
- Native host calls aren't forcibly interrupted by guest deadlines or cancellation.
- Host-side effects, component compilation, and native capability implementations lie outside the guest execution boundary.
- The older raw store and instantiation APIs remain available, but they **do not enforce** an `ExecutionPolicy`.

For strict total-memory or wall-clock containment, use a separately resource-limited worker process. See [Building Projects with Cell](BUILDING_WITH_CELL.md) and the runtime APIs for the enforcement details.

## Use Cell from another Rust project

Cell is available from [GitHub](https://github.com/nate-droid/cell). For reproducible applications, pin every Cell crate to the **same Git revision** and commit your `Cargo.lock`:

```toml
[dependencies]
cell = { git = "https://github.com/nate-droid/cell", tag = "v0.0.1" }
cell-capability-log = { git = "https://github.com/nate-droid/cell", tag = "v0.0.1" }
```

For stricter reproducibility, use `rev = "<full commit SHA>"` in place of `tag`. Additional crates, including `cell-capability-fs`, `cell-capability-openai`, `cell-compose`, and `cell-guest`, are in the same repository. Cargo resolves their internal workspace paths from the Git checkout; you don't need a sibling `../cell` checkout.

The project is experimental. Do not assume the crates are published to crates.io or that the public Rust/WIT interfaces are stable. See [Building Projects with Cell](BUILDING_WITH_CELL.md) for guest bindings and dependency details.

## Explore the repository

| Path | Purpose |
| --- | --- |
| [`crates/cell`](crates/cell) | The small host runtime built on Wasmtime. |
| [`crates/cell-compose`](crates/cell-compose) | Explicit WASM component composition. |
| [`crates/capability-fs`](crates/capability-fs) | Rooted read/write and read-only filesystem implementations. |
| [`crates/capability-log`](crates/capability-log) | Logging interface and native host implementations. |
| [`crates/capability-openai`](crates/capability-openai) | Host-side integration with an OpenAI-compatible chat endpoint. |
| [`crates/guest`](crates/guest) | Guest-side helpers and filesystem bindings. |
| [`examples/`](examples) | Runnable hello, filesystem, and composition demonstrations. |

The OpenAI-compatible capability keeps credentials and network access host-side; it doesn't install ambient networking in the guest.

## Design principles

- **No ambient authority.** Guests receive only explicitly installed interfaces, and capabilities can apply their own operation-level restrictions.
- **Small core, open extensions.** Filesystem, logging, models, and application-specific interfaces belong outside the core runtime.
- **Typed connections.** Prefer declared WIT interfaces and checked component composition over hidden integration.
- **Separation of policy and identity.** Component hashes identify bytes, not trust, permission grants, or execution policies.
- **Be precise about isolation.** Static preflight checks linking, not runtime correctness; managed execution bounds specific resources, not every possible host effect.

Cell currently supports explicit component composition, static inspection, content-addressed artifacts, and managed execution. Inter-Cell communication, independent per-provider quotas, and per-consumer native authority remain future work.

## Development and further reading

From the repository root:

```sh
make smoke
cargo test --workspace --offline
cargo fmt --all -- --check
```

Offline builds require previously cached dependencies. `make smoke` exercises the hello example and core runtime tests, including filesystem grants and denials. Platform-specific test restrictions and known preview limitations are documented in [Developer Preview Readiness](docs/DEVELOPER_PREVIEW_READINESS.md).

- [Building Projects with Cell](BUILDING_WITH_CELL.md) — setup, capability design, WIT, and integration details.
- [Downstream starter](examples/downstream-starter/README.md) — a complete standalone host-and-guest example.
- [Composition demo](examples/compose-demo/README.md) — connect two WASM components through WIT.
- [Release Checklist](RELEASE_CHECKLIST.md) — experimental release milestones and open work.

## License

[MIT](LICENSE). Dependencies have their own licenses.
