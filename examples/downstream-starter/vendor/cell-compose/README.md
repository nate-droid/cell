# cell-compose

Real binary component composition using `wac-graph` 0.12. Providers are embedded
in the output; no callback proxy, automatic wiring, or capability installation.

```rust,no_run
use cell::Cell;
use cell_compose::{Composer, APPLICATION};

# fn main() -> anyhow::Result<()> {
let app = std::fs::read("application.wasm")?;
let provider = std::fs::read("provider.wasm")?;
let plan = Composer::application(app)
    .provider("service", provider)?
    .connect(APPLICATION, "example:service/api@1.0.0", "service", "example:service/api@1.0.0")
    .allow_host_import("service", "example:logging/api@1.0.0");
let bytes: Vec<u8> = plan.compose()?;
let cell = Cell::new()?;
let component = plan.compile(&cell)?;
// Install the declared host interface into the Cell before instantiation.
let mut store = cell.store(());
let instance = cell.instantiate(&mut store, &component)?;
# let _ = (bytes, instance);
# Ok(())
# }
```

## Inspection and validation

- `inspect_component(bytes)` describes a valid binary component's top-level
  imports and exports, including recursive interface members, without a runtime
  engine or composition plan.
- `Composer::describe()` describes the intended graph, including incomplete or
  invalid wiring. Invalid component binaries and a missing application remain
  errors. Unconnected imports distinguish declared host requirements from
  undeclared dependencies; stale host declarations and duplicate connections
  remain visible.
- `Composer::check()` checks complete composition validity using exactly the
  same parsing, wiring, type checking, and encoding path as `compose()`, then
  discards the output. It returns the first validation error.
- `Composer::compose()` produces the composed component.

Descriptions are owned, comparable data with no WAC type/node IDs. Components
are application-first, then providers sorted by name. Imports, exports, host
declarations, unresolved imports, output exports, and connections are sorted
lexically by name or endpoint. Describing a connection does not establish its
validity; unresolved imports mean imports without a declared connection.

Composition validity does not prove runtime host satisfiability: declared host
imports still require implementations in the runtime linker. Inspecting an
already-composed binary reveals its external boundary, not its original provider
names or connection provenance; reconstruction would require metadata that Cell
does not currently embed.

Each `ItemDescription` retains `name` and `kind` and adds `members` (lexically
sorted interface exports, recursively; empty for other kinds) and `function`
(`Some(FunctionDescription)` only for functions). Function summaries contain
named `params` in declaration order, an optional `result`, and `is_async`.
Parameter/result strings use WAC's ID-free type descriptions: primitive types
are precise, but compound and resource types expose only categories such as
`list`, `record`, `own`, or `borrow`. These summaries are not full WIT or proof
of type compatibility; `check()`/`compose()` still use WAC's actual types.
Module and component internals are not expanded.

Both inspection and graph descriptions use the same enriched inventory. Nested
members are inspection data only: connections still select exact top-level
items, and `describe()` still accepts incomplete or incompatible wiring.
Existing field reads remain compatible; downstream `ItemDescription` struct
literals and exhaustive destructuring must account for the two new fields.

## API and rules

- `Composer::application(bytes: impl AsRef<[u8]>) -> Composer`
- `.provider(name: impl Into<String>, bytes: impl AsRef<[u8]>) -> anyhow::Result<Composer>`
- `.connect(consumer, import, provider, export) -> Composer` (all names accept `impl Into<String>`)
- `.allow_host_import(consumer, import) -> Composer`
- `.compose(&self) -> anyhow::Result<Vec<u8>>`
- `.compile<T: 'static>(&self, cell: &Cell<T>) -> anyhow::Result<wasmtime::component::Component>`

`APPLICATION` is the reserved consumer name `"application"`. Provider names are
unique, nonempty, plan-local identifiers. Connections use exact top-level import
and export names, supporting functions or whole interface instances. There is
no nested export-path selection. Only application exports become output exports.

Every import of every supplied component, including unused providers, must have
an explicit connection or a per-consumer host declaration. Host declarations
permit only interface instances, not standalone functions, modules, components,
or types. Unknown declarations, overlapping host/connection declarations,
duplicate connections, incompatible types, and cycles (including self-cycles)
are errors. Endpoint/type/cycle validation happens during `compose` or `compile`;
provider-name validation happens during `provider`.

## Runtime and security caveats

- Inputs are binary components. Parse test/development WAT separately (e.g. with
  `wat::parse_str`). Core modules are not accepted.
- Remaining host imports retain their original names. WAC may merge compatible
  same-name host interfaces; this slice does not rename them or offer isolated
  per-consumer host bindings. Incompatible requirements fail encoding.
- Allowing a host import is not granting a capability. Instantiation still fails
  unless the existing Cell linker supplies it. No ambient imports are allowed.
- Each named provider is instantiated once in each output instance. Multiple
  consumers of that name share its state. Separate output instances have separate
  guest state, even within one Store; host state remains under the caller's control.
- All providers are included, even if unused. There is no pruning, dependency
  discovery, WIT generation, or resource-policy layer in this first slice.
- WAC 0.12 uses wasmparser/encoder 0.258; Wasmtime 47 uses 0.252. Basic component
  composition is tested against the workspace's resolved Wasmtime 47.0.4.
  `compose` validates with WAC, not the target engine: newer component features
  accepted by WAC may still be rejected by Wasmtime. Use `compile` to validate
  runtime compatibility with the actual Cell engine.
- Wasmtime 47 does not implement direct reexports of unresolved imported
  functions. Real applications should call host functions through their guest
  implementation rather than merely reexporting them. The tests use a core guest
  call with canonical lowering/lifting for this path.

Validation: `cargo test -p cell-compose -p cell --offline` and
`cargo clippy -p cell-compose --all-targets --offline -- -D warnings`.
