mod artifact;
pub use artifact::{ArtifactDigest, LocalArtifactStore};

mod provenance;
pub use provenance::{
    ExecutionInputs, ExecutionObservations, ExecutionRecord, RecordedExecution, RecordedLimits,
    values_digest,
};

mod execution;
pub use execution::{
    CancellationToken, ExecutionError, ExecutionErrorKind, ExecutionPolicy, ManagedStore,
};

use wasmtime::{
    Config, Engine, Store,
    component::{Component, Instance, Linker},
};

/// Safe-to-display, advisory metadata for one installed capability.
///
/// This is not an authorization check. Operations must enforce their authority
/// independently. Producers must never include credentials or physical host paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityDescriptor {
    pub interface: &'static str,
    pub authority: AuthorityDescription,
}

/// Structured authority metadata. Additional capability kinds may be added later.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AuthorityDescription {
    /// The capability has not supplied authority metadata; not a denial or grant.
    Unspecified,
    Filesystem {
        roots: Vec<FilesystemRootDescriptor>,
    },
}

/// A logical filesystem root or mount, never a physical host path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilesystemRootDescriptor {
    pub name: String,
    pub read_policy: FilesystemPolicy,
    pub write_policy: FilesystemPolicy,
    /// Operations permitted by the configured policy, not guarantees of success.
    pub operations: Vec<FilesystemOperation>,
}

/// Configured access within a root. Host permissions and runtime checks still apply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FilesystemPolicy {
    Denied,
    WithinRoot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FilesystemOperation {
    ReadText,
    ReadTextRevision,
    Exists,
    SymlinkMetadata,
    ListDir,
    WriteText,
    SaveText,
    CreateDir,
    RemoveFile,
    RemoveDir,
}

/// A host capability that can be installed into a Cell linker.
///
/// Capability crates own their WIT contracts and generated bindings. The Cell
/// core only requires that they know how to register themselves with Wasmtime.
pub trait Capability<T: 'static> {
    /// The public identifier used when listing installed capabilities.
    ///
    /// Override this with a WIT interface name or an application-defined name.
    /// Existing implementations default to their Rust type name.
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Describe configured authority for display only, never for authorization.
    /// Do not include secrets, credentials, or physical host filesystem paths.
    /// The default reports identity only, without implying any authority policy.
    fn descriptor(&self) -> CapabilityDescriptor {
        CapabilityDescriptor {
            interface: self.name(),
            authority: AuthorityDescription::Unspecified,
        }
    }

    fn add_to_linker(self, linker: &mut Linker<T>) -> anyhow::Result<()>;
}

/// A small WebAssembly Component Model runtime with no capabilities by default.
pub struct Cell<T: 'static = ()> {
    engine: Engine,
    linker: Linker<T>,
    capabilities: Vec<&'static str>,
    capability_descriptors: Vec<CapabilityDescriptor>,
}

impl Cell<()> {
    /// Create a Cell with an empty linker and no host capabilities.
    pub fn new() -> anyhow::Result<Self> {
        Self::empty()
    }
}

impl<T> Cell<T>
where
    T: 'static,
{
    /// Create a Cell with an empty linker for the given host-state type.
    pub fn empty() -> anyhow::Result<Self> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.consume_fuel(true);
        config.epoch_interruption(true);

        let engine = Engine::new(&config)?;
        let linker = Linker::new(&engine);

        Ok(Self {
            engine,
            linker,
            capabilities: Vec::new(),
            capability_descriptors: Vec::new(),
        })
    }

    /// Install a capability into this Cell.
    pub fn with<C>(mut self, capability: C) -> anyhow::Result<Self>
    where
        C: Capability<T>,
    {
        let name = capability.name();
        let mut descriptor = capability.descriptor();
        descriptor.interface = name;
        capability
            .add_to_linker(&mut self.linker)
            .map_err(|error| error.context(format!("failed to install capability {name}")))?;
        if !self.capabilities.contains(&name) {
            self.capabilities.push(name);
        }
        self.capability_descriptors.push(descriptor);
        Ok(self)
    }

    /// Advisory display snapshots of successful installations, in installation order.
    ///
    /// Unlike identities, descriptors are not deduplicated. Direct linker changes
    /// are not tracked. Descriptions are not authorization checks and can become
    /// stale for capabilities with mutable policy; runtime enforcement is independent.
    pub fn capability_descriptors(&self) -> &[CapabilityDescriptor] {
        &self.capability_descriptors
    }

    /// List successfully installed capability names in installation order.
    ///
    /// Names are unique. Only installations through [`Self::with`] are tracked;
    /// registrations made directly through [`Self::linker_mut`] are not listed.
    /// This is host-side metadata, not an interface exposed to guests, and does
    /// not describe capability policy (such as filesystem roots or permissions).
    pub fn capabilities(&self) -> &[&'static str] {
        &self.capabilities
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn linker(&self) -> &Linker<T> {
        &self.linker
    }

    pub fn linker_mut(&mut self) -> &mut Linker<T> {
        &mut self.linker
    }

    /// Create a Wasmtime store for this Cell's host-state type.
    pub fn store(&self, state: T) -> Store<T> {
        let mut store = Store::new(&self.engine, state);
        store.set_fuel(u64::MAX).expect("fuel enabled");
        store.set_epoch_deadline(u64::MAX / 2);
        store
    }

    /// Create a store whose execution controls cannot be bypassed through raw store access.
    pub fn managed_store(&self, state: T, policy: ExecutionPolicy) -> ManagedStore<T> {
        ManagedStore::new(&self.engine, state, policy)
    }

    /// Instantiate with managed startup limits, including core start functions.
    pub fn instantiate_managed(
        &self,
        store: &mut ManagedStore<T>,
        component: &Component,
    ) -> Result<Instance, ExecutionError> {
        store.instantiate(self, component)
    }

    /// Invoke an export in a fresh managed store and instance, then drop the store
    /// on both success and failure. Reuse the compiled component across calls.
    /// Supply fresh host state for isolation; shared capability state and external
    /// side effects are not rolled back. Default policy preserves unlimited execution.
    pub fn execute(
        &self,
        component: &Component,
        state: T,
        policy: ExecutionPolicy,
        export: &str,
        params: &[wasmtime::component::Val],
    ) -> Result<Vec<wasmtime::component::Val>, ExecutionError> {
        let mut store = self.managed_store(state, policy);
        let instance = self.instantiate_managed(&mut store, component)?;
        store.call(&instance, export, params)
    }

    /// Verify a stored artifact before compiling it with this Cell's engine.
    /// Artifact identity does not grant authority or select execution policy.
    pub fn component_from_artifact(
        &self,
        artifacts: &LocalArtifactStore,
        digest: ArtifactDigest,
    ) -> anyhow::Result<Component> {
        self.component_from_bytes(artifacts.resolve(digest)?)
    }

    /// Compile component bytes with this Cell's engine.
    pub fn component_from_bytes(&self, bytes: impl AsRef<[u8]>) -> anyhow::Result<Component> {
        let bytes = bytes.as_ref();
        if bytes.starts_with(b"\0asm\x01\0\0\0") {
            anyhow::bail!(
                "expected a WebAssembly component, received a core Wasm module; convert it with `wasm-tools component new` (embedding WIT metadata first if needed)"
            );
        }
        Ok(Component::new(&self.engine, bytes)
            .map_err(|error| error.context("failed to compile WebAssembly component"))?)
    }

    /// Validate all imports against the actual linker without creating a store or
    /// executing guest code. Includes direct linker registrations and checks
    /// interface members and types, not just tracked capability names.
    ///
    /// Success establishes link compatibility only: startup can still trap and
    /// capability policies can still deny individual operations at runtime.
    pub fn check(&self, component: &Component) -> anyhow::Result<()> {
        self.preflight(component).map(|_| ())
    }

    fn preflight(
        &self,
        component: &Component,
    ) -> anyhow::Result<wasmtime::component::InstancePre<T>> {
        self.linker.instantiate_pre(component).map_err(|error| {
            let installed = if self.capabilities.is_empty() {
                "none".to_owned()
            } else {
                self.capabilities.join(", ")
            };
            let error: anyhow::Error = error.into();
            let guidance = linking_guidance(&error);
            error.context(format!(
                "failed to link component: {guidance} Tracked capabilities: {installed}. Direct linker registrations are not listed."
            ))
        })
    }

    /// Instantiate a component using exactly this Cell's installed capabilities.
    pub fn instantiate(
        &self,
        store: &mut Store<T>,
        component: &Component,
    ) -> anyhow::Result<Instance> {
        let pre = self.preflight(component)?;
        Ok(pre.instantiate(store).map_err(|error| {
            error.context(
                "failed to instantiate component after successful linking; inspect the error chain for startup traps, host errors, or resource limits",
            )
        })?)
    }
}

// Wasmtime 47 reports linking failures through textual anyhow contexts rather
// than a public typed missing-import error. Match only its specific diagnostic;
// unknown diagnostics retain neutral guidance and the original error chain.
fn linking_guidance(error: &anyhow::Error) -> String {
    let missing = matches!(
        error.root_cause().to_string().as_str(),
        "function implementation is missing"
            | "module implementation is missing"
            | "resource implementation is missing"
            | "component implementation is missing"
            | "type implementation is missing"
    );
    let interface = error.chain().find_map(|cause| {
        let message = cause.to_string();
        message
            .strip_prefix("component imports instance `")?
            .strip_suffix("`, but a matching implementation was not found in the linker")
            .map(str::to_owned)
    });
    if let (true, Some(interface)) = (missing, interface) {
        let hint = match interface.as_str() {
            "cell:fs/fs@0.1.0" => {
                "Install a filesystem capability such as ReadOnlyFs, LocalFs, or Workspace with .with(...)."
            }
            "cell:log/log@0.1.0" => {
                "Install a logging capability such as StdoutLog with .with(...)."
            }
            "cell:openai/chat@0.1.0" => "Install an OpenAiChat capability with .with(...).",
            name if name.starts_with("wasi:") => {
                "Cell does not install WASI by default. If WASI was unintended, check the guest target and bindings; the starter uses wasm32-unknown-unknown with Cell imports. Otherwise explicitly supply the required WASI interfaces."
            }
            _ => {
                "Supply the required implementation with .with(...) or linker_mut(), or explicitly wire a component provider."
            }
        };
        return format!(
            "unresolved host import `{interface}`: a required implementation is missing (possibly a member of the interface). {hint}"
        );
    }
    "Check the underlying error for missing implementations, incompatible import types or WIT versions, or an engine mismatch. Cell installs no host interfaces automatically.".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use wasmtime::component::Linker;

    struct FlagCapability(Arc<AtomicBool>);

    impl<T: 'static> Capability<T> for FlagCapability {
        fn add_to_linker(self, _linker: &mut Linker<T>) -> anyhow::Result<()> {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FailingCapability;

    impl<T: 'static> Capability<T> for FailingCapability {
        fn add_to_linker(self, _linker: &mut Linker<T>) -> anyhow::Result<()> {
            anyhow::bail!("registration failed")
        }
    }

    fn importing(cell: &Cell, interface: &str) -> anyhow::Result<Component> {
        cell.component_from_bytes(format!(
            r#"(component
                (import "{interface}" (instance
                    (export "read" (func (result string))))))"#
        ))
    }

    #[test]
    fn missing_filesystem_import_has_targeted_guidance() -> anyhow::Result<()> {
        let cell = Cell::new()?;
        let component = importing(&cell, "cell:fs/fs@0.1.0")?;
        let error = cell
            .instantiate(&mut cell.store(()), &component)
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("unresolved host import `cell:fs/fs@0.1.0`")
        );
        assert!(error.to_string().contains("ReadOnlyFs"));
        assert!(format!("{error:#}").contains("function implementation is missing"));
        Ok(())
    }

    #[test]
    fn missing_wasi_import_explains_explicit_authority() -> anyhow::Result<()> {
        let cell = Cell::new()?;
        let component = importing(&cell, "wasi:cli/environment@0.2.0")?;
        let error = cell
            .instantiate(&mut cell.store(()), &component)
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("Cell does not install WASI by default")
        );
        assert!(error.to_string().contains("wasm32-unknown-unknown"));
        Ok(())
    }

    #[test]
    fn incompatible_direct_registration_is_not_reported_as_missing() -> anyhow::Result<()> {
        let mut cell = Cell::new()?;
        cell.linker_mut()
            .instance("cell:fs/fs@0.1.0")?
            .func_wrap("read", |_store, (): ()| Ok(()))?;
        let component = importing(&cell, "cell:fs/fs@0.1.0")?;
        let error = cell
            .instantiate(&mut cell.store(()), &component)
            .err()
            .unwrap();
        assert!(error.to_string().contains("incompatible import types"));
        assert!(!error.to_string().contains("unresolved host import"));
        assert!(!error.to_string().contains("ReadOnlyFs"));
        assert!(format!("{error:#}").contains("instance export `read`"));
        Ok(())
    }

    #[test]
    fn startup_trap_does_not_suggest_installing_capabilities() -> anyhow::Result<()> {
        let cell = Cell::new()?;
        let component = cell.component_from_bytes(
            r#"(component
            (core module $m (func $start unreachable) (start $start))
            (core instance (instantiate $m)))"#,
        )?;
        let error = cell
            .instantiate(&mut cell.store(()), &component)
            .err()
            .unwrap();
        assert!(error.to_string().contains("after successful linking"));
        assert!(!error.to_string().contains(".with("));
        assert!(error.downcast_ref::<wasmtime::Trap>().is_some());
        Ok(())
    }

    #[test]
    fn preflight_checks_actual_linker_without_guest_execution() -> anyhow::Result<()> {
        let mut cell = Cell::new()?;
        let component = importing(&cell, "cell:fs/fs@0.1.0")?;
        let error = cell.check(&component).unwrap_err();
        assert!(error.to_string().contains("ReadOnlyFs"));
        cell.linker_mut()
            .instance("cell:fs/fs@0.1.0")?
            .func_wrap("read", |_store, (): ()| Ok((String::new(),)))?;
        cell.check(&component)?;
        assert!(cell.capabilities().is_empty());
        let startup_trap = cell.component_from_bytes(
            r#"(component
                (core module $m (func $start unreachable) (start $start))
                (core instance (instantiate $m)))"#,
        )?;
        cell.check(&startup_trap)?;
        Ok(())
    }

    #[test]
    fn preflight_rejects_incompatible_interface_member() -> anyhow::Result<()> {
        let mut cell = Cell::new()?;
        cell.linker_mut()
            .instance("cell:fs/fs@0.1.0")?
            .func_wrap("read", |_store, (): ()| Ok(()))?;
        let component = importing(&cell, "cell:fs/fs@0.1.0")?;
        let error = cell.check(&component).unwrap_err();
        assert!(format!("{error:#}").contains("instance export `read`"));
        assert!(!error.to_string().contains("unresolved host import"));
        Ok(())
    }

    #[test]
    fn unknown_link_diagnostic_uses_neutral_guidance() {
        let error = anyhow::anyhow!("an unrecognized linking failure");
        let guidance = linking_guidance(&error);
        assert!(!guidance.contains("unresolved host import"));
        assert!(guidance.contains("underlying error"));
    }

    #[test]
    fn empty_cell_has_no_capabilities() -> anyhow::Result<()> {
        assert!(Cell::new()?.capabilities().is_empty());
        assert!(Cell::<String>::empty()?.capabilities().is_empty());
        Ok(())
    }

    #[test]
    fn capabilities_lists_unique_names_in_installation_order() -> anyhow::Result<()> {
        struct NamedCapability(&'static str);

        impl<T: 'static> Capability<T> for NamedCapability {
            fn name(&self) -> &'static str {
                self.0
            }

            fn add_to_linker(self, _linker: &mut Linker<T>) -> anyhow::Result<()> {
                Ok(())
            }
        }

        let cell = Cell::new()?
            .with(NamedCapability("example:first/api@0.1.0"))?
            .with(NamedCapability("example:second/api@0.1.0"))?
            .with(NamedCapability("example:first/api@0.1.0"))?;
        assert_eq!(
            cell.capabilities(),
            &["example:first/api@0.1.0", "example:second/api@0.1.0"]
        );
        Ok(())
    }

    #[test]
    fn with_installs_capability() -> anyhow::Result<()> {
        let installed = Arc::new(AtomicBool::new(false));

        let cell = Cell::new()?.with(FlagCapability(Arc::clone(&installed)))?;

        assert!(installed.load(Ordering::SeqCst));
        assert_eq!(
            cell.capabilities(),
            &[std::any::type_name::<FlagCapability>()]
        );
        Ok(())
    }

    #[test]
    fn with_returns_capability_registration_errors() {
        let result = Cell::new().unwrap().with(FailingCapability);

        match result {
            Ok(_) => panic!("expected capability registration to fail"),
            Err(error) => {
                assert!(error.to_string().contains("failed to install capability"));
                assert!(format!("{error:#}").contains("registration failed"));
            }
        }
    }

    #[test]
    fn store_contains_supplied_host_state() -> anyhow::Result<()> {
        let cell = Cell::<String>::empty()?;
        let store = cell.store("host state".to_string());

        assert_eq!(store.data(), "host state");
        Ok(())
    }

    #[test]
    fn component_from_bytes_rejects_core_modules_with_conversion_hint() {
        let error = Cell::new()
            .unwrap()
            .component_from_bytes(b"\0asm\x01\0\0\0")
            .err()
            .expect("core modules must be rejected");
        assert!(error.to_string().contains("core Wasm module"));
        assert!(error.to_string().contains("wasm-tools component new"));
    }

    #[test]
    fn component_from_bytes_compiles_and_instantiates_component_text() -> anyhow::Result<()> {
        let cell = Cell::new()?;
        let component = cell.component_from_bytes("(component)")?;
        let mut store = cell.store(());

        cell.instantiate(&mut store, &component)?;

        Ok(())
    }
}
