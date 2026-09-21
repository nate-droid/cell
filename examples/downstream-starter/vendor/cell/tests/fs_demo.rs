use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use cell::Cell;
use cell_capability_fs::{LocalFs, ReadOnlyFs};

const WORKSPACE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
const FS_DEMO_MODULE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../target/wasm32-unknown-unknown/debug/fs_demo.wasm"
);
const FS_DEMO_WIT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/fs-demo/wit");

fn fs_demo_component() -> anyhow::Result<Vec<u8>> {
    build_fs_demo_module()?;

    let mut module = fs::read(FS_DEMO_MODULE)?;
    let mut resolve = wit_parser::Resolve::default();
    let (package, _) = resolve.push_dir(FS_DEMO_WIT)?;
    let world = resolve.select_world(&[package], Some("fs-demo"))?;

    wit_component::embed_component_metadata(
        &mut module,
        &resolve,
        world,
        wit_component::StringEncoding::UTF8,
    )?;

    Ok(wit_component::ComponentEncoder::default()
        .module(&module)?
        .validate(true)
        .encode()?)
}

fn build_fs_demo_module() -> anyhow::Result<()> {
    static BUILD_RESULT: OnceLock<Result<(), String>> = OnceLock::new();

    match BUILD_RESULT.get_or_init(build_fs_demo_module_uncached) {
        Ok(()) => Ok(()),
        Err(error) => anyhow::bail!(error.clone()),
    }
}

fn build_fs_demo_module_uncached() -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(cargo)
        .current_dir(WORKSPACE_ROOT)
        .args([
            "build",
            "-p",
            "fs-demo",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .status()
        .map_err(|error| format!("failed to invoke cargo for fs-demo test fixture: {error}"))?;

    if !status.success() {
        return Err(
            "failed to build fs-demo test fixture for wasm32-unknown-unknown; ensure the target is installed with `rustup target add wasm32-unknown-unknown`"
                .to_string(),
        );
    }

    Ok(())
}

#[test]
fn guest_requiring_fs_cannot_run_without_it() -> anyhow::Result<()> {
    let component = fs_demo_component()?;

    let cell = Cell::new()?;
    let mut store = cell.store(());
    let component = cell.component_from_bytes(component)?;

    let err = cell
        .instantiate(&mut store, &component)
        .expect_err("guest should require the fs capability");

    assert!(
        err.to_string().contains("cell:fs"),
        "unexpected error: {err:#}"
    );

    Ok(())
}

#[test]
fn guest_can_use_local_fs_capability() -> anyhow::Result<()> {
    let component = fs_demo_component()?;
    let dir = TestDir::new()?;
    fs::write(dir.path().join("input.txt"), "hello from host")?;

    let cell = Cell::new()?.with(LocalFs::new(dir.path())?)?;
    let mut store = cell.store(());
    let component = cell.component_from_bytes(component)?;
    let instance = cell.instantiate(&mut store, &component)?;
    let run = instance.get_typed_func::<(), (Result<String, String>,)>(&mut store, "run")?;

    let (result,) = run.call(&mut store, ())?;

    assert_eq!(
        result,
        Ok("input=hello from host; entries=created,input.txt,output.txt".to_string())
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("output.txt"))?,
        "processed: hello from host"
    );
    assert!(dir.path().join("created").is_dir());

    Ok(())
}

#[test]
fn guest_mutation_fails_with_read_only_fs_capability() -> anyhow::Result<()> {
    let component = fs_demo_component()?;
    let dir = TestDir::new()?;
    fs::write(dir.path().join("input.txt"), "hello from host")?;

    let cell = Cell::new()?.with(ReadOnlyFs::new(dir.path())?)?;
    let mut store = cell.store(());
    let component = cell.component_from_bytes(component)?;
    let instance = cell.instantiate(&mut store, &component)?;
    let run = instance.get_typed_func::<(), (Result<String, String>,)>(&mut store, "run")?;

    let (result,) = run.call(&mut store, ())?;

    assert_eq!(
        result,
        Err("write-text \"output.txt\": PermissionDenied".to_string())
    );
    assert!(!dir.path().join("output.txt").exists());
    assert!(!dir.path().join("created").exists());

    Ok(())
}

struct TestDir {
    path: PathBuf,
}

static NEXT_TEST_DIR_ID: AtomicUsize = AtomicUsize::new(0);

impl TestDir {
    fn new() -> anyhow::Result<Self> {
        let id = NEXT_TEST_DIR_ID.fetch_add(1, Ordering::Relaxed);
        let name = format!("cell-fs-demo-test-{}-{id}", std::process::id());
        let path = std::env::temp_dir().join(name);
        fs::create_dir(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
