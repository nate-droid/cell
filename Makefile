.PHONY: smoke hello fs-demo-component external-consumer downstream-starter

WASM_TOOLS ?= wasm-tools

# Python 3.11+; checks only Cargo-listed package inputs in a temporary directory.
external-consumer:
	python3 tests/external-consumer/check.py

# Build and test the standalone host/guest using cached registry dependencies.
downstream-starter:
	python3 examples/downstream-starter/smoke.py . --offline

hello:
	cargo build -p hello --target wasm32-unknown-unknown
	cargo run -p hello-host

smoke: hello
	cargo build -p fs-demo --target wasm32-unknown-unknown
	cargo test -p cell

fs-demo-component:
	@command -v "$(WASM_TOOLS)" >/dev/null 2>&1 || { echo 'Install wasm-tools with: cargo install wasm-tools --locked' >&2; exit 1; }
	cargo build -p fs-demo --target wasm32-unknown-unknown
	$(WASM_TOOLS) component embed examples/fs-demo/wit target/wasm32-unknown-unknown/debug/fs_demo.wasm --world fs-demo -o target/fs-demo.metadata.wasm
	$(WASM_TOOLS) component new target/fs-demo.metadata.wasm -o target/fs-demo.component.wasm
	$(WASM_TOOLS) validate target/fs-demo.component.wasm
