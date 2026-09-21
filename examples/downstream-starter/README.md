# Downstream host + guest starter

This is an **interim local distribution workflow, not a published release**. It does
not assume a canonical remote or registry publication. Obtain a Cell checkout
through your existing process; the repository includes an [MIT license](../../LICENSE).
Review third-party dependency terms separately before redistributing staged inputs.

The starter owns a standalone Cargo workspace containing `host` and `guest`.
Copy this directory wherever you want to develop; it does not inherit the Cell
workspace. Cell dependencies use local `vendor` paths, never repository source
paths. Direct external dependencies are pinned to the checkout's current lockfile
versions: anyhow 1.0.104, wit-component 0.241.2, wit-bindgen 0.48.1. The checkout's
lockfile seeds transitive resolution; keep the generated starter `Cargo.lock` for
repeatable subsequent builds (it is ignored in this example repository).

## Prerequisites and one-command run

- Python 3.11+ (standard library only).
- Cargo/Rust supporting edition 2024, resolver 3, and the checkout's Wasmtime
  requirements; use a current stable toolchain compatible with that checkout.
- Install the guest target: `rustup target add wasm32-unknown-unknown`.
- A local Cell checkout with `Cargo.lock`; registry dependencies must either be
  cached or downloadable from your configured Cargo registry.

From this directory:

```sh
python3 run.py /absolute/path/to/cell
```

For an entirely cached/offline build, append `--offline`. Staging is always offline
and does not fetch Cell source. Without `--offline`, builds may contact configured
Cargo registries to download third-party dependencies. No API keys, external
services, or network authority are granted to the guest.

`run.py` uses `cargo metadata` and `cargo package --list --allow-dirty --offline`
to stage public crates from `crates/`, following `tests/external-consumer/check.py`.
It resolves workspace package/dependency inheritance, retains dependency options,
rewrites local paths to sibling staged packages, and removes repository-only dev
dependencies. Only Cargo-listed inputs are copied (generated Cargo metadata is
replaced by resolved manifests). This is source staging, not registry vendoring:
third-party crates still come from Cargo's registry/cache. Uncommitted source is
allowed, so reproducibility requires recording the checkout revision and changes.

WIT is automatically copied from **staged** logging/filesystem capability inputs
into `guest/wit/deps`; do not manually copy WIT or edit generated directories.
Re-running replaces staged packages and WIT. `--stage-only` stops before building.
All build products and the generated fixture stay in this directory's `target`.
The supplied checkout is used only as input; no checkout target directory is used.

The script builds a core guest module for `wasm32-unknown-unknown`, then runs the
native host with its wasm path and a read-only fixture root. The host explicitly
uses `ComponentEncoder` with validation, grants only `StdoutLog` and `ReadOnlyFs`,
calls typed `run: func() -> result<string, string>`. Wasmtime 47 performs canonical
ABI post-return cleanup automatically; no deprecated `post_return` call is needed.
The guest
reads `input.txt`, logs startup, and verifies that writing `denied.txt` returns
permission-denied. Expect:

```text
downstream guest started
read=hello from the downstream fixture; write=permission-denied
```

To run against your own root after building:

```sh
cargo run -p downstream-host --target-dir target -- target/wasm32-unknown-unknown/debug/downstream_guest.wasm /path/to/read-only-root
```

That root must contain `input.txt`. The host takes a core wasm module with embedded
WIT metadata, not an already-encoded component.

## Smoke validation

```sh
python3 smoke.py /absolute/path/to/cell --offline
```

The smoke script copies authored inputs into a fresh standalone directory under
`smoke-work`, bootstraps and builds there, checks output and write denial, compares
staged WIT, verifies workspace/path isolation, and tests usage and missing-input
errors. It removes the isolated build afterward. Failures return nonzero. This is
a real integration build and may take several minutes and substantial disk space.

## Troubleshooting

- Missing `tomllib`: use Python 3.11 or newer.
- Target/core library missing: install `wasm32-unknown-unknown` for the active Rust
  toolchain. Do not substitute a WASI target; no WASI adapter is used here.
- Offline package missing: run without `--offline` with registry access. Staging
  itself requires the checkout's metadata/package listing to work offline.
- Compiler/Wasmtime requirement failure: use a compatible newer Rust toolchain.
- WIT/import mismatch: rerun bootstrap against one consistent checkout; do not
  mix separately copied WIT and crates.
- Updating Cell or its dependency versions: review the exact pins, remove the
  generated starter lockfile if reseeding is intended, and rerun smoke validation.
  Staging preserves upstream version constraints, not arbitrary new version pins.
- Guest read error: check `input.txt` in the supplied root. Write denial is expected.
- Encoding error: rebuild the guest with wit-bindgen metadata intact and pass the
  guest core module path, not a component or unrelated wasm file.

For integration, add this smoke command to CI with a pinned checkout/toolchain and
cached registry dependencies. A future release should define real distribution
coordinates and matching Cargo license metadata, publish/version public crates and WIT together, then
replace interim local staging with that supported distribution mechanism.
