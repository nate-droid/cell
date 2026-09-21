use cell::Cell;

const HELLO_COMPONENT: &str = r#"
(component
  (import "cell:log/log@0.1.0" (instance
    (export "log" (func (param "message" string)))
  ))
)
"#;

#[test]
fn guest_requiring_log_cannot_run_without_it() -> anyhow::Result<()> {
    let component = HELLO_COMPONENT;

    let cell = Cell::new()?;
    let mut store = cell.store(());
    let x = cell.component_from_bytes(component)?;

    let err = cell
        .instantiate(&mut store, &x)
        .expect_err("guest should require the log capability");

    assert!(
        err.to_string().contains("cell:log"),
        "unexpected error: {err:#}"
    );

    assert!(err.to_string().contains("Tracked capabilities: none"));
    assert!(err.to_string().contains(".with(...)"));
    assert!(err.to_string().contains("StdoutLog"));
    assert!(err.to_string().contains("unresolved host import"));
    assert!(!err.to_string().contains("WASI"));
    assert!(format!("{err:#}").contains("function implementation is missing"));

    Ok(())
}
