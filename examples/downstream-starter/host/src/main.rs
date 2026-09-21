use anyhow::{bail, Context, Result};
use cell::Cell;
use cell_capability_fs::ReadOnlyFs;
use cell_capability_log::StdoutLog;
use std::{env, fs, path::PathBuf};

fn main() -> Result<()> {
    let args: Vec<PathBuf> = env::args_os().skip(1).map(PathBuf::from).collect();
    if args.len() != 2 {
        bail!("usage: downstream-host <guest.wasm> <read-only-root>");
    }
    let module = fs::read(&args[0]).context("reading guest wasm")?;
    let bytes = wit_component::ComponentEncoder::default()
        .module(&module)
        .context("guest must be a core wasm module with embedded WIT metadata")?
        .validate(true)
        .encode()
        .context("encoding and validating guest component")?;
    let cell = Cell::new()?
        .with(StdoutLog)?
        .with(ReadOnlyFs::new(&args[1])?)?;
    let component = cell.component_from_bytes(bytes)?;
    let mut store = cell.store(());
    let instance = cell.instantiate(&mut store, &component)?;
    let run = instance.get_typed_func::<(), (Result<String, String>,)>(&mut store, "run")?;
    let (result,) = run
        .call(&mut store, ())
        .map_err(|error| error.context("calling guest run"))?;
    // Wasmtime 47 performs canonical ABI post-return cleanup automatically.
    let message = result.map_err(|error| anyhow::anyhow!("guest: {error}"))?;
    println!("{message}");
    Ok(())
}
