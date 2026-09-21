# Formatter composition demo

Two Rust guests communicate through WIT, not through a host callback:

- `../formatter-provider` exports `cell-example:formatter/formatter@0.1.0`, whose
  `format: func(input: string) -> string` returns `formatted: <input>`.
- `../formatter-app` imports that interface and exports
  `run: func(input: string) -> string`, forwarding its input to the provider.
- `compose-demo` reads both binary components from explicit CLI paths, connects
  the interface with `cell-compose::Composer`, and compiles and executes the
  resulting component in an empty `Cell<()>`. No capabilities or host imports
  are installed or allowed; unresolved imports fail composition.

The provider's WIT is vendored in the application's `wit/deps/formatter`;
keep both copies in sync when changing the interface.

## Build and run

Run these commands from the workspace root. Install the Rust target and
[`wasm-tools`](https://github.com/bytecodealliance/wasm-tools) if necessary:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-tools --locked

cargo build -p formatter-provider -p formatter-app \
  --target wasm32-unknown-unknown --release

wasm-tools component new \
  target/wasm32-unknown-unknown/release/formatter_provider.wasm \
  -o target/formatter-provider.component.wasm
wasm-tools component new \
  target/wasm32-unknown-unknown/release/formatter_app.wasm \
  -o target/formatter-app.component.wasm

cargo run -p compose-demo -- \
  target/formatter-app.component.wasm \
  target/formatter-provider.component.wasm \
  'hello composition'
```

Expected stdout:

```text
formatted: hello composition
```

Rust emits core modules with `wit-bindgen` component metadata. The
`wasm-tools component new` step turns them into the binary components required
by `Composer`; do not pass the core modules directly to the runner. Using
`wasm32-unknown-unknown` avoids ambient WASI imports and requires no WASI adapter.
The input is a required UTF-8 argument (quote it to preserve spaces; `''` is
valid). Artifact paths may be absolute or relative to the current directory.

Host validation, without cross-compiling the guests:

```sh
cargo test --workspace
cargo clippy -p compose-demo --all-targets -- -D warnings
```
