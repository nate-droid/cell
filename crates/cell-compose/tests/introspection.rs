use cell_compose::{
    APPLICATION, ComponentRole, Composer, Endpoint, FunctionDescription, ItemDescription, ItemKind,
    inspect_component,
};

fn wasm(source: &str) -> Vec<u8> {
    wat::parse_str(source).unwrap()
}

fn app() -> Vec<u8> {
    wasm(
        r#"(component
        (import "dep" (instance $dep (export "next" (func (result u32)))))
        (alias export $dep "next" (func $next))
        (export "run" (func $next)))"#,
    )
}

fn provider() -> Vec<u8> {
    wasm(
        r#"(component
        (core module $m (func (export "next") (result i32) i32.const 1))
        (core instance $m (instantiate $m))
        (func $next (result u32) (canon lift (core func $m "next")))
        (instance $api (export "next" (func $next)))
        (export "api" (instance $api)))"#,
    )
}

fn forward() -> Vec<u8> {
    wasm(
        r#"(component
        (import "dep" (instance $dep (export "next" (func (result u32)))))
        (export "api" (instance $dep)))"#,
    )
}

fn endpoint(component: &str, item: &str) -> Endpoint {
    Endpoint {
        component: component.into(),
        item: item.into(),
    }
}

fn rejected(plan: &Composer, message: &str) {
    let check = format!("{:#}", plan.check().unwrap_err());
    let compose = format!("{:#}", plan.compose().unwrap_err());
    assert_eq!(check, compose);
    assert!(check.contains(message), "{check}");
    assert!(plan.describe().is_ok());
}

#[test]
fn inspect_reports_boundary_and_sorts_items() {
    let description = inspect_component(wasm(
        r#"(component
        (import "z" (func $z))
        (import "a" (instance))
        (export "z-out" (func $z))
        (export "a-out" (func $z)))"#,
    ))
    .unwrap();
    assert_eq!(
        description.imports,
        vec![
            ItemDescription {
                name: "a".into(),
                kind: ItemKind::Interface,
                members: vec![],
                function: None,
            },
            ItemDescription {
                name: "z".into(),
                kind: ItemKind::Function,
                members: vec![],
                function: Some(FunctionDescription {
                    params: vec![],
                    result: None,
                    is_async: false,
                }),
            },
        ]
    );
    assert_eq!(
        description.exports,
        vec![
            ItemDescription {
                name: "a-out".into(),
                kind: ItemKind::Function,
                members: vec![],
                function: Some(FunctionDescription {
                    params: vec![],
                    result: None,
                    is_async: false,
                }),
            },
            ItemDescription {
                name: "z-out".into(),
                kind: ItemKind::Function,
                members: vec![],
                function: Some(FunctionDescription {
                    params: vec![],
                    result: None,
                    is_async: false,
                }),
            },
        ]
    );
    assert!(inspect_component(wasm("(module)")).is_err());
    assert!(inspect_component(b"not wasm").is_err());
    let mut truncated = provider();
    truncated.pop();
    assert!(inspect_component(truncated).is_err());
}

#[test]
fn incomplete_graph_is_describable_but_not_valid() {
    let plan = Composer::application(app())
        .provider("counter", provider())
        .unwrap();
    let graph = plan.describe().unwrap();
    assert_eq!(graph.components.len(), 2);
    assert_eq!(graph.components[0].name, APPLICATION);
    assert_eq!(graph.components[0].role, ComponentRole::Application);
    assert_eq!(
        graph.components[0].component,
        inspect_component(app()).unwrap()
    );
    assert_eq!(graph.components[1].name, "counter");
    assert_eq!(graph.components[1].role, ComponentRole::Provider);
    assert_eq!(
        graph.components[1].component,
        inspect_component(provider()).unwrap()
    );
    assert_eq!(graph.output_exports, vec![endpoint(APPLICATION, "run")]);
    assert_eq!(graph.unresolved_imports.len(), 1);
    assert_eq!(
        graph.unresolved_imports[0].endpoint,
        endpoint(APPLICATION, "dep")
    );
    assert_eq!(graph.unresolved_imports[0].kind, ItemKind::Interface);
    assert!(!graph.unresolved_imports[0].host_declared);
    rejected(&plan, "unresolved import");
}

fn chain(reverse: bool) -> Composer {
    let mut plan = Composer::application(app());
    if reverse {
        plan = plan
            .provider("forward", forward())
            .unwrap()
            .provider("counter", provider())
            .unwrap();
        plan = plan.connect("forward", "dep", "counter", "api").connect(
            APPLICATION,
            "dep",
            "forward",
            "api",
        );
    } else {
        plan = plan
            .provider("counter", provider())
            .unwrap()
            .provider("forward", forward())
            .unwrap();
        plan = plan
            .connect(APPLICATION, "dep", "forward", "api")
            .connect("forward", "dep", "counter", "api");
    }
    plan
}

#[test]
fn valid_provider_chain_has_deterministic_inventory_and_connections() {
    let plan = chain(false);
    let graph = plan.describe().unwrap();
    assert_eq!(graph, chain(true).describe().unwrap());
    assert!(graph.unresolved_imports.is_empty());
    assert_eq!(
        graph
            .components
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>(),
        vec![APPLICATION, "counter", "forward"]
    );
    assert_eq!(graph.connections[0].consumer, endpoint(APPLICATION, "dep"));
    assert_eq!(graph.connections[0].provider, endpoint("forward", "api"));
    assert_eq!(graph.connections[1].consumer, endpoint("forward", "dep"));
    assert_eq!(graph.connections[1].provider, endpoint("counter", "api"));
    plan.check().unwrap();
    inspect_component(plan.compose().unwrap()).unwrap();
}

#[test]
fn host_requirements_are_described_without_claiming_runtime_satisfaction() {
    let plan = Composer::application(app())
        .provider("forward", forward())
        .unwrap()
        .connect(APPLICATION, "dep", "forward", "api")
        .allow_host_import("forward", "dep");
    let graph = plan.describe().unwrap();
    assert_eq!(graph.host_declarations, vec![endpoint("forward", "dep")]);
    assert_eq!(graph.unresolved_imports.len(), 1);
    assert_eq!(
        graph.unresolved_imports[0].endpoint,
        endpoint("forward", "dep")
    );
    assert!(graph.unresolved_imports[0].host_declared);
    plan.check().unwrap();
    let composed = inspect_component(plan.compose().unwrap()).unwrap();
    assert_eq!(
        composed.imports,
        vec![ItemDescription {
            name: "dep".into(),
            kind: ItemKind::Interface,
            members: vec![ItemDescription {
                name: "next".into(),
                kind: ItemKind::Function,
                members: vec![],
                function: Some(FunctionDescription {
                    params: vec![],
                    result: Some("u32".into()),
                    is_async: false,
                }),
            }],
            function: None,
        }]
    );
}

#[test]
fn stale_and_invalid_host_declarations_remain_visible() {
    let stale = Composer::application(app())
        .allow_host_import("unknown", "missing")
        .allow_host_import(APPLICATION, "missing");
    assert_eq!(
        stale.describe().unwrap().host_declarations,
        vec![
            endpoint(APPLICATION, "missing"),
            endpoint("unknown", "missing")
        ]
    );
    rejected(&stale, "unknown host import");

    let invalid = Composer::application(wasm(r#"(component (import "f" (func)))"#))
        .allow_host_import(APPLICATION, "f");
    let graph = invalid.describe().unwrap();
    assert!(graph.unresolved_imports[0].host_declared);
    assert_eq!(graph.unresolved_imports[0].kind, ItemKind::Function);
    rejected(&invalid, "must be an interface");

    let overlap = chain(false).allow_host_import(APPLICATION, "dep");
    assert_eq!(
        overlap.describe().unwrap().host_declarations,
        vec![endpoint(APPLICATION, "dep")]
    );
    rejected(&overlap, "also declared as host");
}

#[test]
fn check_and_compose_reject_type_mismatches_and_cycles() {
    let wrong = wasm(
        r#"(component
        (core module $m (func (export "next") (result i64) i64.const 0))
        (core instance $m (instantiate $m))
        (func $next (result u64) (canon lift (core func $m "next")))
        (instance $api (export "next" (func $next)))
        (export "api" (instance $api)))"#,
    );
    let mismatch = Composer::application(app())
        .provider("wrong", wrong)
        .unwrap()
        .connect(APPLICATION, "dep", "wrong", "api");
    rejected(&mismatch, "connecting");
    for self_cycle in [true, false] {
        let mut plan = Composer::application(app())
            .provider("a", forward())
            .unwrap()
            .connect(APPLICATION, "dep", "a", "api");
        if self_cycle {
            plan = plan.connect("a", "dep", "a", "api");
        } else {
            plan = plan
                .provider("b", forward())
                .unwrap()
                .connect("a", "dep", "b", "api")
                .connect("b", "dep", "a", "api");
        }
        rejected(&plan, "dependency cycles");
    }
}

#[test]
fn missing_application_and_invalid_components_are_not_describable() {
    assert!(Composer::default().describe().is_err());
    let invalid = Composer::application(app())
        .provider("broken", b"invalid")
        .unwrap();
    assert!(invalid.describe().is_err());
    assert!(invalid.check().is_err());
}
