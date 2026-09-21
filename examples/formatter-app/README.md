# Formatter application

A `wit-bindgen` guest importing `cell-example:formatter/formatter@0.1.0` and
exporting `run(input: string) -> string`. Its implementation calls the imported
formatter, so it must be explicitly connected to a provider before execution.

See [the composition demo](../compose-demo/README.md) for complete
`wasm32-unknown-unknown` build, component encoding, and run instructions.
