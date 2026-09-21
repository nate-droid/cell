use anyhow::{Context, Result, bail};
use cell::Cell;
use cell_compose::{APPLICATION, Composer};
use std::{env, fs, path::PathBuf};

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let usage =
        "usage: compose-demo <application.component.wasm> <provider.component.wasm> <input>";
    let application = PathBuf::from(args.next().context(usage)?);
    let provider = PathBuf::from(args.next().context(usage)?);
    let input = args
        .next()
        .context(usage)?
        .into_string()
        .map_err(|_| anyhow::anyhow!("input must be valid UTF-8"))?;
    if args.next().is_some() {
        bail!("{usage}");
    }

    let application_bytes = fs::read(&application)
        .with_context(|| format!("reading application {}", application.display()))?;
    let provider_bytes =
        fs::read(&provider).with_context(|| format!("reading provider {}", provider.display()))?;
    let plan = Composer::application(application_bytes)
        .provider("formatter", provider_bytes)?
        .connect(
            APPLICATION,
            "cell-example:formatter/formatter@0.1.0",
            "formatter",
            "cell-example:formatter/formatter@0.1.0",
        );

    let cell = Cell::new()?;
    let component = plan.compile(&cell)?;
    let mut store = cell.store(());
    let instance = cell.instantiate(&mut store, &component)?;
    let run = instance.get_typed_func::<(String,), (String,)>(&mut store, "run")?;
    let (output,) = run.call(&mut store, (input,))?;

    println!("{output}");
    Ok(())
}
