use anyhow::Result;
use cell::Cell;
use cell_compose::{APPLICATION, Composer};

fn bytes(wat: &str) -> Vec<u8> {
    wat::parse_str(wat).unwrap()
}

fn application() -> Vec<u8> {
    bytes(
        r#"(component
        (import "dep" (instance $dep (export "next" (func (result u32)))))
        (alias export $dep "next" (func $next))
        (core func $lowered (canon lower (func $next)))
        (core module $caller
            (import "dep" "next" (func $next (result i32)))
            (func (export "run") (result i32) call $next))
        (core instance $imports (export "next" (func $lowered)))
        (core instance $caller (instantiate $caller (with "dep" (instance $imports))))
        (func $run (result u32) (canon lift (core func $caller "run")))
        (export "run" (func $run)))"#,
    )
}

fn counter() -> Vec<u8> {
    bytes(
        r#"(component
        (core module $m
            (global $count (mut i32) (i32.const 0))
            (func (export "next") (result i32)
                global.get $count i32.const 1 i32.add global.set $count global.get $count))
        (core instance $m (instantiate $m))
        (func $next (result u32) (canon lift (core func $m "next")))
        (instance $api (export "next" (func $next)))
        (export "api" (instance $api)))"#,
    )
}

fn plan() -> Composer {
    Composer::application(application())
        .provider("counter", counter())
        .unwrap()
        .connect(APPLICATION, "dep", "counter", "api")
}

#[test]
fn real_composition_calls_and_instances_have_independent_state() -> Result<()> {
    let cell = Cell::new()?;
    let encoded = plan().compose()?;
    assert!(encoded.starts_with(b"\0asm"));
    let component = cell.component_from_bytes(encoded)?;
    let mut store = cell.store(());
    let first = cell.instantiate(&mut store, &component)?;
    let second = cell.instantiate(&mut store, &component)?;
    let a = first.get_typed_func::<(), (u32,)>(&mut store, "run")?;
    let b = second.get_typed_func::<(), (u32,)>(&mut store, "run")?;
    for expected in [1, 2] {
        assert_eq!(a.call(&mut store, ())?, (expected,));
    }
    assert_eq!(b.call(&mut store, ())?, (1,));

    plan().compile(&cell)?;
    Ok(())
}

#[test]
fn missing_connection_is_not_auto_wired() {
    let err = Composer::application(application())
        .provider("dep", counter())
        .unwrap()
        .compose()
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("unresolved import `application` / `dep`")
    );
}

#[test]
fn mismatch_is_rejected() {
    let wrong = bytes(
        r#"(component
        (core module $m (func (export "next") (result i64) i64.const 0))
        (core instance $m (instantiate $m))
        (func $f (result u64) (canon lift (core func $m "next")))
        (instance $i (export "next" (func $f)))
        (export "api" (instance $i)))"#,
    );
    let err = Composer::application(application())
        .provider("wrong", wrong)
        .unwrap()
        .connect(APPLICATION, "dep", "wrong", "api")
        .compose()
        .unwrap_err();
    assert!(format!("{err:#}").contains("connecting `application`"));
}

fn forwarding_provider() -> Vec<u8> {
    bytes(
        r#"(component
        (import "dep" (instance $dep (export "next" (func (result u32)))))
        (export "api" (instance $dep)))"#,
    )
}

#[test]
fn provider_dependencies_must_be_resolved() {
    let err = Composer::application(application())
        .provider("p", forwarding_provider())
        .unwrap()
        .connect(APPLICATION, "dep", "p", "api")
        .compose()
        .unwrap_err();
    assert!(err.to_string().contains("unresolved import `p` / `dep`"));
}

#[test]
fn explicit_provider_chain_works() -> Result<()> {
    let cell = Cell::new()?;
    let component = Composer::application(application())
        .provider("forward", forwarding_provider())?
        .provider("counter", counter())?
        .connect(APPLICATION, "dep", "forward", "api")
        .connect("forward", "dep", "counter", "api")
        .compile(&cell)?;
    let mut store = cell.store(());
    let instance = cell.instantiate(&mut store, &component)?;
    let run = instance.get_typed_func::<(), (u32,)>(&mut store, "run")?;
    assert_eq!(run.call(&mut store, ())?, (1,));
    Ok(())
}

#[test]
fn cycles_and_self_cycles_are_rejected() {
    for self_cycle in [false, true] {
        let mut plan = Composer::application(application())
            .provider("a", forwarding_provider())
            .unwrap()
            .connect(APPLICATION, "dep", "a", "api");
        if self_cycle {
            plan = plan.connect("a", "dep", "a", "api");
        } else {
            plan = plan
                .provider("b", forwarding_provider())
                .unwrap()
                .connect("a", "dep", "b", "api")
                .connect("b", "dep", "a", "api");
        }
        assert!(plan.compose().is_err());
    }
}

#[test]
fn declared_host_interface_survives_and_requires_linker_registration() -> Result<()> {
    for provider_host in [false, true] {
        let mut plan = Composer::application(application());
        if provider_host {
            plan = plan
                .provider("forward", forwarding_provider())?
                .connect(APPLICATION, "dep", "forward", "api")
                .allow_host_import("forward", "dep");
        } else {
            plan = plan.allow_host_import(APPLICATION, "dep");
        }
        let mut cell = Cell::new()?;
        let component = plan.compile(&cell)?;
        let mut store = cell.store(());
        assert!(cell.instantiate(&mut store, &component).is_err());
        assert!(cell.capabilities().is_empty());
        cell.linker_mut()
            .instance("dep")?
            .func_wrap("next", |_, (): ()| Ok((42u32,)))?;
        let instance = cell.instantiate(&mut store, &component)?;
        let run = instance.get_typed_func::<(), (u32,)>(&mut store, "run")?;
        assert_eq!(run.call(&mut store, ())?, (42,));
    }
    Ok(())
}

#[test]
fn invalid_plans_fail_closed() {
    assert!(Composer::default().compose().is_err());
    assert!(Composer::application(b"bad").compose().is_err());
    assert!(Composer::application(bytes("(module)")).compose().is_err());
    assert!(
        Composer::application(application())
            .provider(APPLICATION, counter())
            .is_err()
    );
    assert!(plan().provider("counter", counter()).is_err());
    assert!(
        plan()
            .connect(APPLICATION, "dep", "counter", "api")
            .compose()
            .is_err()
    );
    assert!(
        plan()
            .allow_host_import(APPLICATION, "dep")
            .compose()
            .is_err()
    );
    assert!(
        plan()
            .allow_host_import("counter", "absent")
            .compose()
            .is_err()
    );
    assert!(
        Composer::application(application())
            .allow_host_import("absent", "dep")
            .compose()
            .is_err()
    );
    for (consumer, import, provider, export) in [
        ("absent", "dep", "counter", "api"),
        (APPLICATION, "absent", "counter", "api"),
        (APPLICATION, "dep", "absent", "api"),
        (APPLICATION, "dep", "counter", "absent"),
        (APPLICATION, "dep", APPLICATION, "run"),
    ] {
        assert!(
            Composer::application(application())
                .provider("counter", counter())
                .unwrap()
                .connect(consumer, import, provider, export)
                .compose()
                .is_err()
        );
    }
    let direct = bytes(r#"(component (import "f" (func (result u32))))"#);
    assert!(
        Composer::application(direct)
            .allow_host_import(APPLICATION, "f")
            .compose()
            .is_err()
    );
}

#[test]
fn empty_application_is_valid() -> Result<()> {
    let cell = Cell::new()?;
    let component = Composer::application(bytes("(component)")).compile(&cell)?;
    cell.instantiate(&mut cell.store(()), &component)?;
    Ok(())
}
