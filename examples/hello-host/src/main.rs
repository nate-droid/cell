use anyhow::{Context, Result, bail};
use cell::Cell;
use cell_capability_log::StdoutLog;
use std::{env, fs, path::PathBuf};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/wasm32-unknown-unknown/debug/hello.wasm"));
    if args.next().is_some() {
        bail!("usage: hello-host [hello.component.wasm]");
    }
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "reading {}; run `make hello` from the repository root to build and run the example",
            path.display()
        )
    })?;
    let bytes = if bytes.starts_with(b"\0asm\x01\0\0\0") {
        wit_component::ComponentEncoder::default()
            .module(&bytes)?
            .validate(true)
            .encode()
            .context("encoding hello module as a component; WIT metadata must be embedded")?
    } else {
        bytes
    };
    let cell = Cell::new()?.with(StdoutLog)?;
    let component = cell.component_from_bytes(bytes)?;
    let mut store = cell.store(());
    let instance = cell.instantiate(&mut store, &component)?;
    let run = instance
        .get_typed_func::<(), ()>(&mut store, "run")
        .map_err(|error| error.context("hello component must export run: func()"))?;
    run.call(&mut store, ())
        .map_err(|error| error.context("calling hello's run export"))?;

    Ok(())
}
