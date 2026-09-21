//! Explicit, fail-closed WebAssembly component composition.
//!
//! Providers are embedded components, not host callbacks. All dependencies must
//! be connected explicitly or declared as remaining host interface imports.
//! Composition does not install capabilities into a [`cell::Cell`]. Validation
//! cannot guarantee that a runtime linker satisfies remaining host imports.
//!
//! - [`inspect_component`] describes a component's boundary and recursive interface members.
//! - [`Composer::describe`] describes the intended graph, including incomplete wiring.
//! - [`Composer::check`] validates complete composition, including encoding.
//! - [`Composer::compose`] produces the composed binary.
//!
//! Inspecting a composed binary cannot reconstruct original provider names or
//! connection provenance without metadata that Cell does not currently embed.

use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use wac_graph::{
    CompositionGraph, EncodeOptions, NodeId,
    types::{ItemKind as WacItemKind, Package},
};

/// Reserved consumer name for the application.
pub const APPLICATION: &str = "application";

/// The kind of a component item, independent of WAC type identities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemKind {
    Function,
    Interface,
    Module,
    Component,
    Type,
    Value,
}

impl ItemKind {
    fn from_wac(kind: WacItemKind) -> Self {
        match kind {
            WacItemKind::Func(_) => Self::Function,
            WacItemKind::Instance(_) => Self::Interface,
            WacItemKind::Module(_) => Self::Module,
            WacItemKind::Component(_) => Self::Component,
            WacItemKind::Type(_) => Self::Type,
            WacItemKind::Value(_) => Self::Value,
        }
    }
}

/// An ID-free function signature summary, not a structural compatibility check.
/// Type strings are WAC descriptions: primitives are precise, while compound
/// and resource types report only their category (e.g. `list`, `own`, `borrow`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionDescription {
    /// Named parameters in declaration order, not lexical order.
    pub params: Vec<(String, String)>,
    /// `None` means the function has no result.
    pub result: Option<String>,
    pub is_async: bool,
}

/// A named import, export, or recursive interface member.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemDescription {
    pub name: String,
    pub kind: ItemKind,
    /// Lexically ordered exports for interfaces; empty for all other kinds.
    pub members: Vec<ItemDescription>,
    /// Present only for functions.
    pub function: Option<FunctionDescription>,
}

/// Lexically ordered top-level inventories of a validated component.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentDescription {
    pub imports: Vec<ItemDescription>,
    pub exports: Vec<ItemDescription>,
}

/// A component's role in a composition plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentRole {
    Application,
    Provider,
}

/// A plan-local component and its inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComponentNode {
    pub name: String,
    pub role: ComponentRole,
    pub component: ComponentDescription,
}

/// An exact top-level item in a plan-local component.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Endpoint {
    pub component: String,
    pub item: String,
}

/// A declared connection; description does not validate its endpoints or types.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Connection {
    pub consumer: Endpoint,
    pub provider: Endpoint,
}

/// An actual import with no declared connection, whether or not allowed as host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnresolvedImport {
    pub endpoint: Endpoint,
    pub kind: ItemKind,
    pub host_declared: bool,
}

/// A plan snapshot, not a guarantee of valid wiring or runtime satisfiability.
/// Components are application-first then lexical; other inventories are lexical.
/// Connections retain duplicates and host declarations retain stale endpoints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphDescription {
    pub components: Vec<ComponentNode>,
    pub connections: Vec<Connection>,
    pub host_declarations: Vec<Endpoint>,
    pub unresolved_imports: Vec<UnresolvedImport>,
    pub output_exports: Vec<Endpoint>,
}

fn describe_item(graph: &CompositionGraph, name: &str, kind: WacItemKind) -> ItemDescription {
    let types = graph.types();
    let mut members = match kind {
        WacItemKind::Instance(id) => types[id]
            .exports
            .iter()
            .map(|(name, kind)| describe_item(graph, name, *kind))
            .collect(),
        _ => Vec::new(),
    };
    members.sort_by(|a, b| a.name.cmp(&b.name));
    let function = match kind {
        WacItemKind::Func(id) => {
            let ty = &types[id];
            Some(FunctionDescription {
                params: ty
                    .params
                    .iter()
                    .map(|(name, ty)| (name.clone(), ty.desc(types).to_owned()))
                    .collect(),
                result: ty.result.map(|ty| ty.desc(types).to_owned()),
                is_async: ty.is_async,
            })
        }
        _ => None,
    };
    ItemDescription {
        name: name.to_owned(),
        kind: ItemKind::from_wac(kind),
        members,
        function,
    }
}

fn inventory(graph: &CompositionGraph, package: &Package) -> ComponentDescription {
    let world = &graph.types()[package.ty()];
    let items = |mut items: Vec<ItemDescription>| {
        items.sort_by(|a, b| a.name.cmp(&b.name));
        items
    };
    ComponentDescription {
        imports: items(
            world
                .imports
                .iter()
                .map(|(name, kind)| describe_item(graph, name, *kind))
                .collect(),
        ),
        exports: items(
            world
                .exports
                .iter()
                .map(|(name, kind)| describe_item(graph, name, *kind))
                .collect(),
        ),
    }
}

/// Validate and inspect a binary component without a runtime engine or linker.
/// This cannot recover component names, connections, or host declaration provenance.
pub fn inspect_component(bytes: impl AsRef<[u8]>) -> Result<ComponentDescription> {
    let mut graph = CompositionGraph::new();
    let package = Package::from_bytes(
        "cell:inspection",
        None,
        bytes.as_ref().to_vec(),
        graph.types_mut(),
    )
    .context("invalid component")?;
    Ok(inventory(&graph, &package))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &[u8] = b"\0asm\x0d\0\x01\0";

    #[test]
    fn inspection_validates_component_binaries() {
        let description = inspect_component(EMPTY).unwrap();
        assert!(description.imports.is_empty());
        assert!(description.exports.is_empty());
        assert!(inspect_component(b"\0asm\x01\0\0\0").is_err());
        assert!(inspect_component(b"not wasm").is_err());
    }

    #[test]
    fn inspection_exposes_recursive_interface_members() {
        let bytes = wat::parse_str(
            r#"(component
            (type $leaf (instance
                (type $f (func (param "z" u32) (param "a" string) (result bool)))
                (export "z-call" (func (type $f)))
                (export "a-call" (func))))
            (type $outer (instance
                (export "z-inner" (instance (type $leaf)))
                (export "a-call" (func))))
            (import "api" (instance $api (type $outer)))
            (export "api" (instance $api)))"#,
        )
        .unwrap();
        let description = inspect_component(&bytes).unwrap();
        assert_eq!(description.imports, description.exports);
        let api = &description.imports[0];
        assert_eq!(api.kind, ItemKind::Interface);
        assert!(api.function.is_none());
        assert_eq!(
            api.members
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a-call", "z-inner"]
        );
        let leaf = &api.members[1];
        assert_eq!(
            leaf.members
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a-call", "z-call"]
        );
        let call = &leaf.members[1];
        assert!(call.members.is_empty());
        assert_eq!(
            call.function,
            Some(FunctionDescription {
                params: vec![("z".into(), "u32".into()), ("a".into(), "string".into())],
                result: Some("bool".into()),
                is_async: false,
            })
        );
        assert_eq!(leaf.members[0].function.as_ref().unwrap().result, None);
        let composer = Composer::application(&bytes);
        let plan = composer.describe().unwrap();
        assert_eq!(plan.components[0].component, description);
        assert_eq!(plan.unresolved_imports.len(), 1);
        assert!(!plan.unresolved_imports[0].host_declared);
        assert!(composer.check().is_err());
    }

    #[test]
    fn composition_preflight_checks_host_linker_without_startup() -> Result<()> {
        let bytes = wat::parse_str(
            r#"(component
                (import "api" (instance (export "call" (func))))
                (core module $m (func $start unreachable) (start $start))
                (core instance (instantiate $m)))"#,
        )?;
        let composer = Composer::application(bytes).allow_host_import(APPLICATION, "api");
        composer.check()?;
        let mut cell = cell::Cell::new()?;
        assert!(composer.check_with(&cell).is_err());
        cell.linker_mut()
            .instance("api")?
            .func_wrap("call", |_store, (): ()| Ok(()))?;
        composer.check_with(&cell)?;
        Ok(())
    }

    #[test]
    fn same_named_incompatible_interfaces_remain_describable() {
        let app = wat::parse_str(
            r#"(component
            (import "api" (instance (export "call" (func (result u32))))))"#,
        )
        .unwrap();
        let provider = wat::parse_str(
            r#"(component
            (core module $m (func (export "call") (result i64) i64.const 0))
            (core instance $m (instantiate $m))
            (func $call (result u64) (canon lift (core func $m "call")))
            (instance $api (export "call" (func $call)))
            (export "api" (instance $api)))"#,
        )
        .unwrap();
        let composer = Composer::application(app)
            .provider("service", provider)
            .unwrap()
            .connect(APPLICATION, "api", "service", "api");
        let plan = composer.describe().unwrap();
        assert_eq!(plan.connections.len(), 1);
        assert!(plan.unresolved_imports.is_empty());
        assert_ne!(
            plan.components[0].component.imports[0].members,
            plan.components[1].component.exports[0].members
        );
        let error = composer.check().unwrap_err().to_string();
        assert!(error.contains("connecting"), "{error}");
        assert_eq!(error, composer.compose().unwrap_err().to_string());
    }

    #[test]
    fn description_retains_invalid_declarations_and_duplicates() {
        let composer = Composer::application(EMPTY)
            .provider("z", EMPTY)
            .unwrap()
            .provider("a", EMPTY)
            .unwrap()
            .connect(APPLICATION, "missing", "z", "missing")
            .connect(APPLICATION, "missing", "z", "missing")
            .connect("a", "missing", "unknown", "missing")
            .allow_host_import("stale", "missing");
        let description = composer.describe().unwrap();
        assert_eq!(
            description
                .components
                .iter()
                .map(|node| node.name.as_str())
                .collect::<Vec<_>>(),
            vec![APPLICATION, "a", "z"]
        );
        assert_eq!(description.connections.len(), 3);
        assert_eq!(description.connections[1], description.connections[2]);
        assert_eq!(
            description.host_declarations,
            vec![Endpoint {
                component: "stale".into(),
                item: "missing".into(),
            }]
        );
        assert!(description.unresolved_imports.is_empty());
        assert!(composer.check().is_err());
        assert_eq!(
            composer.check().unwrap_err().to_string(),
            composer.compose().unwrap_err().to_string()
        );
        assert!(Composer::application(EMPTY).check().is_ok());
        assert!(Composer::default().describe().is_err());
    }
}

struct Prepared {
    graph: CompositionGraph,
    nodes: BTreeMap<String, NodeId>,
    imports: BTreeMap<(String, String), WacItemKind>,
    components: Vec<ComponentNode>,
    app_exports: Vec<String>,
}

/// A composition plan. Inputs must be binary-encoded components, not core modules
/// or WAT. Every named provider is instantiated once per composed instance;
/// consumers sharing a provider therefore share its state within that instance.
#[derive(Default)]
pub struct Composer {
    application: Option<Vec<u8>>,
    providers: BTreeMap<String, Vec<u8>>,
    connections: Vec<(String, String, String, String)>,
    hosts: BTreeSet<(String, String)>,
}

impl Composer {
    /// Start a plan with the application whose exports become the output exports.
    pub fn application(bytes: impl AsRef<[u8]>) -> Self {
        Self {
            application: Some(bytes.as_ref().to_vec()),
            ..Self::default()
        }
    }

    /// Add a uniquely named provider. Names are plan-local, not WIT names.
    pub fn provider(mut self, name: impl Into<String>, bytes: impl AsRef<[u8]>) -> Result<Self> {
        let name = name.into();
        if name.is_empty() || name == APPLICATION || self.providers.contains_key(&name) {
            bail!("invalid or duplicate provider name `{name}`");
        }
        self.providers.insert(name, bytes.as_ref().to_vec());
        Ok(self)
    }

    /// Connect a consumer's exact top-level import to a named provider's exact
    /// top-level export. Use [`APPLICATION`] for the application consumer.
    /// Functions and whole interfaces are supported; there is no name-based wiring.
    pub fn connect(
        mut self,
        consumer: impl Into<String>,
        import: impl Into<String>,
        provider: impl Into<String>,
        export: impl Into<String>,
    ) -> Self {
        self.connections.push((
            consumer.into(),
            import.into(),
            provider.into(),
            export.into(),
        ));
        self
    }

    /// Permit an exact unconnected interface import on this consumer to remain
    /// an outer host import under its original name. This grants no capability:
    /// the Cell linker must still supply it. Non-interface imports are rejected.
    /// Unused declarations and declarations overlapping connections are errors.
    pub fn allow_host_import(
        mut self,
        consumer: impl Into<String>,
        import: impl Into<String>,
    ) -> Self {
        self.hosts.insert((consumer.into(), import.into()));
        self
    }

    /// Encode a real composed component with embedded providers. Reject invalid
    /// components, unknown endpoints, duplicate connections, type mismatches,
    /// unresolved dependencies, and dependency cycles.
    pub fn compose(&self) -> Result<Vec<u8>> {
        let Prepared {
            mut graph,
            nodes,
            imports,
            app_exports,
            ..
        } = self.prepare()?;
        self.encode(&mut graph, &nodes, &imports, app_exports)
    }

    /// Perform the same full encoding validation as [`Self::compose`], discarding bytes.
    /// Success does not guarantee a runtime linker can supply the host imports.
    pub fn check(&self) -> Result<()> {
        self.compose().map(|_| ())
    }

    /// Validate composition and remaining host imports against a Cell's actual
    /// linker, without instantiating providers or the application.
    /// Unlike [`Self::check`], this also detects missing host capabilities and
    /// incompatible host interface implementations. No store or host state is needed.
    pub fn check_with<T: 'static>(&self, cell: &cell::Cell<T>) -> Result<()> {
        let component = self.compile(cell)?;
        cell.check(&component)
    }

    /// Describe valid input binaries even when wiring is incomplete or invalid.
    /// Missing application and invalid component binaries remain errors.
    pub fn describe(&self) -> Result<GraphDescription> {
        let prepared = self.prepare()?;
        let endpoint = |component: &String, item: &String| Endpoint {
            component: component.clone(),
            item: item.clone(),
        };
        let mut connections: Vec<_> = self
            .connections
            .iter()
            .map(|(consumer, import, provider, export)| Connection {
                consumer: endpoint(consumer, import),
                provider: endpoint(provider, export),
            })
            .collect();
        connections.sort();
        let connected: BTreeSet<_> = self
            .connections
            .iter()
            .map(|(consumer, import, _, _)| (consumer.clone(), import.clone()))
            .collect();
        let unresolved_imports = prepared
            .imports
            .iter()
            .filter(|(key, _)| !connected.contains(*key))
            .map(|((component, item), kind)| UnresolvedImport {
                endpoint: endpoint(component, item),
                kind: ItemKind::from_wac(*kind),
                host_declared: self.hosts.contains(&(component.clone(), item.clone())),
            })
            .collect();
        let output_exports = prepared.components[0]
            .component
            .exports
            .iter()
            .map(|item| Endpoint {
                component: APPLICATION.into(),
                item: item.name.clone(),
            })
            .collect();
        Ok(GraphDescription {
            components: prepared.components,
            connections,
            host_declarations: self
                .hosts
                .iter()
                .map(|(component, item)| endpoint(component, item))
                .collect(),
            unresolved_imports,
            output_exports,
        })
    }

    fn prepare(&self) -> Result<Prepared> {
        let application = self.application.as_ref().context("missing application")?;
        let mut graph = CompositionGraph::new();
        let mut nodes = BTreeMap::new();
        let mut imports = BTreeMap::new();
        let mut app_exports = Vec::new();
        let mut components = Vec::new();
        for (index, (name, bytes)) in std::iter::once((APPLICATION, application))
            .chain(
                self.providers
                    .iter()
                    .map(|(name, bytes)| (name.as_str(), bytes)),
            )
            .enumerate()
        {
            // Package identities are internal and independent of user-supplied names.
            let package = Package::from_bytes(
                &format!("cell:component-{index}"),
                None,
                bytes.clone(),
                graph.types_mut(),
            )
            .with_context(|| format!("invalid component `{name}`"))?;
            components.push(ComponentNode {
                name: name.to_owned(),
                role: if name == APPLICATION {
                    ComponentRole::Application
                } else {
                    ComponentRole::Provider
                },
                component: inventory(&graph, &package),
            });
            let world = &graph.types()[package.ty()];
            for (import, kind) in &world.imports {
                imports.insert((name.to_owned(), import.clone()), *kind);
            }
            if name == APPLICATION {
                app_exports = world.exports.keys().cloned().collect();
            }
            let package = graph.register_package(package)?;
            nodes.insert(name.to_owned(), graph.instantiate(package));
        }
        Ok(Prepared {
            graph,
            nodes,
            imports,
            components,
            app_exports,
        })
    }

    fn encode(
        &self,
        graph: &mut CompositionGraph,
        nodes: &BTreeMap<String, NodeId>,
        imports: &BTreeMap<(String, String), WacItemKind>,
        app_exports: Vec<String>,
    ) -> Result<Vec<u8>> {
        let mut connected = BTreeSet::new();
        for (consumer, import, provider, export) in &self.connections {
            let key = (consumer.clone(), import.clone());
            if !connected.insert(key.clone()) {
                bail!("duplicate connection for `{consumer}` import `{import}`");
            }
            if self.hosts.contains(&key) {
                bail!("connected import `{consumer}` / `{import}` is also declared as host");
            }
            let consumer_node = *nodes
                .get(consumer)
                .with_context(|| format!("unknown consumer `{consumer}`"))?;
            if provider == APPLICATION {
                bail!("application cannot be used as a provider");
            }
            let provider_node = *nodes
                .get(provider)
                .with_context(|| format!("unknown provider `{provider}`"))?;
            let alias = graph
                .alias_instance_export(provider_node, export)
                .with_context(|| format!("provider `{provider}` export `{export}`"))?;
            graph.set_instantiation_argument(consumer_node, import, alias)
                .with_context(|| format!("connecting `{consumer}` import `{import}` to `{provider}` export `{export}`"))?;
        }
        for key in &self.hosts {
            match imports.get(key) {
                Some(WacItemKind::Instance(_)) => {}
                Some(_) => bail!("host import `{}` / `{}` must be an interface", key.0, key.1),
                None => bail!("unknown host import `{}` / `{}`", key.0, key.1),
            }
        }
        for key in imports.keys() {
            if !connected.contains(key) && !self.hosts.contains(key) {
                bail!(
                    "unresolved import `{}` / `{}`: connect it or explicitly allow a host interface",
                    key.0,
                    key.1
                );
            }
        }
        for export in app_exports {
            let alias = graph.alias_instance_export(nodes[APPLICATION], &export)?;
            graph.export(alias, export)?;
        }
        graph
            .encode(EncodeOptions::default())
            .context("encoding composition (dependency cycles are not allowed)")
    }

    /// Compose and compile using exactly this Cell's engine. Instantiation still
    /// uses the Cell's existing linker and requires any declared host interfaces.
    pub fn compile<T: 'static>(
        &self,
        cell: &cell::Cell<T>,
    ) -> Result<wasmtime::component::Component> {
        cell.component_from_bytes(self.compose()?)
    }
}
