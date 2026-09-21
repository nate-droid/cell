use crate::Cell;
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use wasmtime::{
    Engine, ResourceLimiter, Store, Trap, UpdateDeadline,
    component::{Component, Instance, Val},
};

/// Cooperative cancellation, scoped to stores using this token. Cancellation is sticky.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);
impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Limits for managed synchronous execution. None means no Cell-imposed limit.
/// Fuel is refreshed for each instantiation/call; call and post-return share a budget.
/// Deadline is absolute and cancellation is sticky. Memory is a per-linear-memory
/// byte ceiling, not a bound on host allocations or total process memory.
#[derive(Clone, Debug, Default)]
pub struct ExecutionPolicy {
    pub fuel: Option<u64>,
    pub memory_bytes: Option<usize>,
    pub deadline: Option<Instant>,
    pub cancellation: Option<CancellationToken>,
    /// Recursive logical payload bytes (each node costs at least one byte).
    pub input_bytes: Option<usize>,
    pub output_bytes: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionErrorKind {
    Trap,
    Fuel,
    Memory,
    Deadline,
    Cancelled,
    Invalid,
    Input,
    Output,
}

#[derive(Debug)]
pub struct ExecutionError {
    pub kind: ExecutionErrorKind,
    source: anyhow::Error,
}
impl ExecutionError {
    fn new(kind: ExecutionErrorKind, message: &str) -> Self {
        Self {
            kind,
            source: anyhow::anyhow!("{message}"),
        }
    }
    pub(crate) fn classify(source: impl Into<anyhow::Error>) -> Self {
        let source = source.into();
        let kind = if let Some(e) = source.downcast_ref::<Self>() {
            e.kind
        } else if matches!(source.downcast_ref::<Trap>(), Some(Trap::OutOfFuel)) {
            ExecutionErrorKind::Fuel
        } else if source.downcast_ref::<Trap>().is_some() {
            ExecutionErrorKind::Trap
        } else {
            ExecutionErrorKind::Invalid
        };
        Self { kind, source }
    }
}
impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.source)
    }
}
impl std::error::Error for ExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

impl ExecutionPolicy {
    fn check(&self) -> Result<(), ExecutionError> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(ExecutionError::new(
                ExecutionErrorKind::Cancelled,
                "execution cancelled",
            ));
        }
        if self.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(ExecutionError::new(
                ExecutionErrorKind::Deadline,
                "execution deadline elapsed",
            ));
        }
        Ok(())
    }
}
struct MemoryLimit(Option<usize>);
impl ResourceLimiter for MemoryLimit {
    fn memory_growing(
        &mut self,
        _: usize,
        desired: usize,
        _: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if self.0.is_some_and(|limit| desired > limit) {
            return Err(ExecutionError::new(
                ExecutionErrorKind::Memory,
                "guest linear memory ceiling exceeded",
            )
            .into());
        }
        Ok(true)
    }
    fn table_growing(&mut self, _: usize, _: usize, _: Option<usize>) -> wasmtime::Result<bool> {
        Ok(true)
    }
}

/// Owns the store and controls without changing capability host state T.
/// Native host functions cannot be forcibly interrupted: deadline/cancellation are
/// checked before and after execution and at guest epoch safepoints. Wasmtime 47
/// component calls do not dispatch Store call hooks. Host code must implement its
/// own I/O timeouts/cooperative cancellation. Output bounds are checked after
/// Wasmtime lifts results, so they do not prevent the initial host allocation.
/// After execution fails inside Wasmtime, this store rejects further execution
/// (canonical ABI state may be poisoned). Use a fresh instance/store on the same
/// Cell, or Cell::execute for invocation isolation. Host side effects are not rolled back.
pub struct ManagedStore<T: 'static> {
    store: Store<T>,
    policy: ExecutionPolicy,
    poisoned: bool,
    fuel_budget: Option<u64>,
}
impl<T: 'static> ManagedStore<T> {
    pub(crate) fn new(engine: &Engine, state: T, policy: ExecutionPolicy) -> Self {
        let mut store = Store::new(engine, state);
        let mut limiter = Box::new(MemoryLimit(policy.memory_bytes));
        store.limiter(move |_| {
            // SAFETY: the closure owns this stable allocation for the store lifetime.
            // Wasmtime accesses the returned reference only during its exclusive
            // store borrow; no other access to this private allocation exists.
            unsafe { &mut *(&mut *limiter as *mut MemoryLimit) }
        });
        let checks = policy.clone();
        store.epoch_deadline_callback(move |_| {
            checks.check()?;
            Ok(UpdateDeadline::Continue(1))
        });
        Self {
            store,
            policy,
            poisoned: false,
            fuel_budget: None,
        }
    }
    pub fn data(&self) -> &T {
        self.store.data()
    }
    pub fn data_mut(&mut self) -> &mut T {
        self.store.data_mut()
    }
    pub fn policy(&self) -> &ExecutionPolicy {
        &self.policy
    }
    /// Whether guest execution failed and this store must be discarded.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }
    fn check_ready(&self) -> Result<(), ExecutionError> {
        if self.poisoned {
            return Err(ExecutionError::new(
                ExecutionErrorKind::Invalid,
                "managed store is poisoned; create a fresh store and instance",
            ));
        }
        self.policy.check()
    }
    pub(crate) fn measured_fuel(&self) -> Option<u64> {
        self.fuel_budget.and_then(|budget| {
            self.store
                .get_fuel()
                .ok()
                .and_then(|remaining| budget.checked_sub(remaining))
        })
    }

    pub(crate) fn clear_fuel_measurement(&mut self) {
        self.fuel_budget = None;
    }

    fn begin(&mut self) -> Result<Option<Ticker>, ExecutionError> {
        self.check_ready()?;
        self.store
            .set_fuel(self.policy.fuel.unwrap_or(u64::MAX))
            .map_err(ExecutionError::classify)?;
        self.fuel_budget = Some(self.policy.fuel.unwrap_or(u64::MAX));
        let timed = self.policy.deadline.is_some() || self.policy.cancellation.is_some();
        self.store
            .set_epoch_deadline(if timed { 1 } else { u64::MAX / 2 });
        Ok(timed.then(|| Ticker::new(self.store.engine().clone())))
    }
    pub(crate) fn instantiate(
        &mut self,
        cell: &Cell<T>,
        component: &Component,
    ) -> Result<Instance, ExecutionError> {
        let _ticker = self.begin()?;
        let pre = cell
            .preflight(component)
            .map_err(ExecutionError::classify)?;
        let instance = pre.instantiate(&mut self.store).map_err(|error| {
            self.poisoned = true;
            ExecutionError::classify(error)
        })?;
        self.policy.check()?;
        Ok(instance)
    }
    /// Call a top-level dynamic export and always perform post-return after a
    /// successful call, including when output validation fails. No typed-call or
    /// mutable raw-store escape hatch is exposed by this wrapper.
    pub fn call(
        &mut self,
        instance: &Instance,
        export: &str,
        params: &[Val],
    ) -> Result<Vec<Val>, ExecutionError> {
        self.check_ready()?;
        payload(params, self.policy.input_bytes, ExecutionErrorKind::Input)?;
        let func = instance.get_func(&mut self.store, export).ok_or_else(|| {
            ExecutionError::new(ExecutionErrorKind::Invalid, "function export not found")
        })?;
        let mut results = vec![Val::Bool(false); func.ty(&self.store).results().len()];
        let _ticker = self.begin()?;
        func.call(&mut self.store, params, &mut results)
            .map_err(|error| {
                self.poisoned = true;
                ExecutionError::classify(error)
            })?;
        let output = payload(
            &results,
            self.policy.output_bytes,
            ExecutionErrorKind::Output,
        );
        // Wasmtime 47 executes canonical post-return inside Func::call.
        self.policy.check()?;
        output?;
        Ok(results)
    }
}

// Epoch increments are shared notifications, never unconditional cancellation.
// Each store callback checks only its own policy. Joining prevents orphan tickers.
struct Ticker {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Ticker {
    fn new(engine: Engine) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            while !flag.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(1));
                engine.increment_epoch();
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.thread.take().unwrap().join();
    }
}

fn payload(
    values: &[Val],
    limit: Option<usize>,
    kind: ExecutionErrorKind,
) -> Result<(), ExecutionError> {
    let Some(limit) = limit else {
        return Ok(());
    };
    let mut remaining = limit;
    let mut pending: Vec<&Val> = values.iter().collect();
    while let Some(value) = pending.pop() {
        let mut cost = 1usize;
        match value {
            Val::String(s) | Val::Enum(s) => cost = cost.saturating_add(s.len()),
            Val::List(v) | Val::Tuple(v) => pending.extend(v),
            Val::Map(v) => {
                for (k, v) in v {
                    pending.push(k);
                    pending.push(v);
                }
            }
            Val::Record(v) => {
                for (k, v) in v {
                    cost = cost.saturating_add(k.len());
                    pending.push(v);
                }
            }
            Val::Variant(k, v) => {
                cost = cost.saturating_add(k.len());
                pending.extend(v.as_deref());
            }
            Val::Option(v) => pending.extend(v.as_deref()),
            Val::Result(Ok(v) | Err(v)) => pending.extend(v.as_deref()),
            Val::Flags(v) => {
                for s in v {
                    cost = cost.saturating_add(s.len());
                }
            }
            Val::S16(_) | Val::U16(_) => cost = 2,
            Val::S32(_) | Val::U32(_) | Val::Float32(_) | Val::Char(_) => cost = 4,
            Val::S64(_) | Val::U64(_) | Val::Float64(_) => cost = 8,
            _ => {}
        }
        remaining = remaining
            .checked_sub(cost)
            .ok_or_else(|| ExecutionError::new(kind, "dynamic value payload ceiling exceeded"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn guest(cell: &Cell, body: &str) -> Component {
        cell.component_from_bytes(format!(
            r#"(component
            (core module $m (func (export "run") {body}))
            (core instance $i (instantiate $m))
            (func (export "run") (canon lift (core func $i "run"))))"#
        ))
        .unwrap()
    }
    fn recovery(cell: &Cell) {
        let mut store = cell.managed_store((), ExecutionPolicy::default());
        let instance = cell
            .instantiate_managed(&mut store, &guest(cell, ""))
            .unwrap();
        assert!(store.call(&instance, "run", &[]).unwrap().is_empty());
    }
    #[test]
    fn failed_store_rejects_execution_but_fresh_execution_recovers() {
        let cell = Cell::new().unwrap();
        let component = guest(&cell, "unreachable");
        let mut store = cell.managed_store((), ExecutionPolicy::default());
        let instance = cell.instantiate_managed(&mut store, &component).unwrap();
        assert_eq!(
            store.call(&instance, "run", &[]).unwrap_err().kind,
            ExecutionErrorKind::Trap
        );
        assert!(store.is_poisoned());
        assert_eq!(
            store.call(&instance, "run", &[]).unwrap_err().kind,
            ExecutionErrorKind::Invalid
        );
        assert_eq!(
            cell.instantiate_managed(&mut store, &component)
                .unwrap_err()
                .kind,
            ExecutionErrorKind::Invalid
        );
        assert_eq!(
            cell.execute(&component, (), ExecutionPolicy::default(), "run", &[])
                .unwrap_err()
                .kind,
            ExecutionErrorKind::Trap
        );
        let healthy = guest(&cell, "");
        for _ in 0..3 {
            assert!(
                cell.execute(&healthy, (), ExecutionPolicy::default(), "run", &[])
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn isolated_execution_resets_guest_state() {
        let cell = Cell::new().unwrap();
        let component = cell
            .component_from_bytes(
                r#"(component
            (core module $m
                (global $count (mut i32) (i32.const 0))
                (func (export "run") (result i32)
                    global.get $count i32.const 1 i32.add
                    global.set $count global.get $count))
            (core instance $i (instantiate $m))
            (func (export "run") (result u32)
                (canon lift (core func $i "run"))))"#,
            )
            .unwrap();
        for _ in 0..3 {
            let results = cell
                .execute(&component, (), ExecutionPolicy::default(), "run", &[])
                .unwrap();
            assert!(matches!(results.as_slice(), [Val::U32(1)]));
        }
        let mut store = cell.managed_store((), ExecutionPolicy::default());
        let instance = cell.instantiate_managed(&mut store, &component).unwrap();
        for count in 1..=3 {
            let results = store.call(&instance, "run", &[]).unwrap();
            assert!(matches!(results.as_slice(), [Val::U32(value)] if *value == count));
        }
    }

    #[test]
    fn validation_failure_does_not_poison_store() {
        let cell = Cell::new().unwrap();
        let component = guest(&cell, "");
        let mut store = cell.managed_store((), ExecutionPolicy::default());
        let instance = cell.instantiate_managed(&mut store, &component).unwrap();
        assert_eq!(
            store.call(&instance, "missing", &[]).unwrap_err().kind,
            ExecutionErrorKind::Invalid
        );
        assert!(!store.is_poisoned());
        for _ in 0..3 {
            assert!(store.call(&instance, "run", &[]).unwrap().is_empty());
        }
    }

    #[test]
    fn post_return_shares_fuel_budget() {
        let cell = Cell::new().unwrap();
        let component = cell
            .component_from_bytes(
                r#"(component
            (core module $m
                (func (export "run"))
                (func (export "cleanup") (loop br 0)))
            (core instance $i (instantiate $m))
            (func (export "run") (canon lift (core func $i "run")
                (post-return (core func $i "cleanup")))))"#,
            )
            .unwrap();
        let mut store = cell.managed_store(
            (),
            ExecutionPolicy {
                fuel: Some(1000),
                ..Default::default()
            },
        );
        let instance = cell.instantiate_managed(&mut store, &component).unwrap();
        assert_eq!(
            store.call(&instance, "run", &[]).unwrap_err().kind,
            ExecutionErrorKind::Fuel
        );
        recovery(&cell);
    }
    #[test]
    fn shared_epoch_ticks_do_not_cancel_other_stores() {
        let cell = Cell::new().unwrap();
        let component = guest(&cell, "(loop br 0)");
        let token = CancellationToken::default();
        let mut cancelled = cell.managed_store(
            (),
            ExecutionPolicy {
                cancellation: Some(token.clone()),
                ..Default::default()
            },
        );
        let instance = cell
            .instantiate_managed(&mut cancelled, &component)
            .unwrap();
        let mut other = cell.managed_store(
            (),
            ExecutionPolicy {
                deadline: Some(Instant::now() + Duration::from_millis(100)),
                ..Default::default()
            },
        );
        let other_instance = cell.instantiate_managed(&mut other, &component).unwrap();
        let worker =
            std::thread::spawn(move || cancelled.call(&instance, "run", &[]).unwrap_err().kind);
        let cancel = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            token.cancel();
        });
        assert_eq!(
            other.call(&other_instance, "run", &[]).unwrap_err().kind,
            ExecutionErrorKind::Deadline
        );
        assert_eq!(worker.join().unwrap(), ExecutionErrorKind::Cancelled);
        cancel.join().unwrap();
        recovery(&cell);
    }
    #[test]
    fn startup_deadline_and_pre_cancelled() {
        let cell = Cell::new().unwrap();
        let component = cell
            .component_from_bytes(
                r#"(component
            (core module $m (func $s (loop br 0)) (start $s))
            (core instance (instantiate $m)))"#,
            )
            .unwrap();
        let mut store = cell.managed_store(
            (),
            ExecutionPolicy {
                deadline: Some(Instant::now() + Duration::from_millis(20)),
                ..Default::default()
            },
        );
        assert_eq!(
            cell.instantiate_managed(&mut store, &component)
                .unwrap_err()
                .kind,
            ExecutionErrorKind::Deadline
        );
        let token = CancellationToken::default();
        token.cancel();
        let mut store = cell.managed_store(
            (),
            ExecutionPolicy {
                cancellation: Some(token),
                ..Default::default()
            },
        );
        assert_eq!(
            cell.instantiate_managed(&mut store, &component)
                .unwrap_err()
                .kind,
            ExecutionErrorKind::Cancelled
        );
        recovery(&cell);
    }
    #[test]
    fn unlimited_defaults_and_generic_state() {
        let cell = Cell::<String>::empty().unwrap();
        let mut store = cell.managed_store("state".into(), ExecutionPolicy::default());
        store.data_mut().push('!');
        assert_eq!(store.data(), "state!");
        let p = store.policy();
        assert!(
            p.fuel.is_none()
                && p.memory_bytes.is_none()
                && p.deadline.is_none()
                && p.cancellation.is_none()
                && p.input_bytes.is_none()
                && p.output_bytes.is_none()
        );
    }
    #[test]
    fn startup_and_invocation_fuel() {
        let cell = Cell::new().unwrap();
        let startup = cell
            .component_from_bytes(
                r#"(component
            (core module $m (func $s (loop br 0)) (start $s))
            (core instance (instantiate $m)))"#,
            )
            .unwrap();
        let mut store = cell.managed_store(
            (),
            ExecutionPolicy {
                fuel: Some(1000),
                ..Default::default()
            },
        );
        assert_eq!(
            cell.instantiate_managed(&mut store, &startup)
                .unwrap_err()
                .kind,
            ExecutionErrorKind::Fuel
        );
        let mut store = cell.managed_store(
            (),
            ExecutionPolicy {
                fuel: Some(1000),
                ..Default::default()
            },
        );
        let instance = cell
            .instantiate_managed(&mut store, &guest(&cell, "(loop br 0)"))
            .unwrap();
        assert_eq!(
            store.call(&instance, "run", &[]).unwrap_err().kind,
            ExecutionErrorKind::Fuel
        );
        // Wasmtime may poison canonical ABI flags after a trap; use a fresh store.
        let mut store = cell.managed_store(
            (),
            ExecutionPolicy {
                fuel: Some(1000),
                ..Default::default()
            },
        );
        let instance = cell
            .instantiate_managed(&mut store, &guest(&cell, ""))
            .unwrap();
        store.call(&instance, "run", &[]).unwrap();
        recovery(&cell);
    }
    #[test]
    fn memory_startup_and_growth() {
        let cell = Cell::new().unwrap();
        let startup = cell
            .component_from_bytes(
                r#"(component (core module $m (memory 2))
            (core instance (instantiate $m)))"#,
            )
            .unwrap();
        let mut store = cell.managed_store(
            (),
            ExecutionPolicy {
                memory_bytes: Some(65536),
                ..Default::default()
            },
        );
        assert_eq!(
            cell.instantiate_managed(&mut store, &startup)
                .unwrap_err()
                .kind,
            ExecutionErrorKind::Memory
        );
        let growth = cell
            .component_from_bytes(
                r#"(component
            (core module $m (memory 1) (func (export "run") i32.const 1 memory.grow drop))
            (core instance $i (instantiate $m))
            (func (export "run") (canon lift (core func $i "run"))))"#,
            )
            .unwrap();
        assert!(store.is_poisoned());
        let mut store = cell.managed_store(
            (),
            ExecutionPolicy {
                memory_bytes: Some(65536),
                ..Default::default()
            },
        );
        let instance = cell.instantiate_managed(&mut store, &growth).unwrap();
        assert_eq!(
            store.call(&instance, "run", &[]).unwrap_err().kind,
            ExecutionErrorKind::Memory
        );
        recovery(&cell);
    }
    #[test]
    fn deadlines_and_cancellation_interrupt_and_recover() {
        let cell = Cell::new().unwrap();
        let infinite = guest(&cell, "(loop br 0)");
        let mut store = cell.managed_store(
            (),
            ExecutionPolicy {
                deadline: Some(Instant::now() + Duration::from_millis(50)),
                ..Default::default()
            },
        );
        let instance = cell.instantiate_managed(&mut store, &infinite).unwrap();
        assert_eq!(
            store.call(&instance, "run", &[]).unwrap_err().kind,
            ExecutionErrorKind::Deadline
        );
        recovery(&cell);
        let token = CancellationToken::default();
        let mut store = cell.managed_store(
            (),
            ExecutionPolicy {
                cancellation: Some(token.clone()),
                ..Default::default()
            },
        );
        let instance = cell.instantiate_managed(&mut store, &infinite).unwrap();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            token.cancel();
        });
        assert_eq!(
            store.call(&instance, "run", &[]).unwrap_err().kind,
            ExecutionErrorKind::Cancelled
        );
        thread.join().unwrap();
        recovery(&cell);
    }
    #[test]
    fn recursive_payload_and_validation() {
        let nested = Val::Option(Some(Box::new(Val::List(vec![Val::String("é".into())]))));
        let values = [Val::Record(vec![("x".into(), nested)])];
        assert!(payload(&values, Some(7), ExecutionErrorKind::Input).is_ok());
        assert_eq!(
            payload(&values, Some(6), ExecutionErrorKind::Input)
                .unwrap_err()
                .kind,
            ExecutionErrorKind::Input
        );
        let cell = Cell::new().unwrap();
        let mut store = cell.managed_store(
            (),
            ExecutionPolicy {
                input_bytes: Some(0),
                ..Default::default()
            },
        );
        let instance = cell
            .instantiate_managed(&mut store, &guest(&cell, ""))
            .unwrap();
        assert_eq!(
            store
                .call(&instance, "run", &[Val::String("x".into())])
                .unwrap_err()
                .kind,
            ExecutionErrorKind::Input
        );
        assert_eq!(
            store.call(&instance, "missing", &[]).unwrap_err().kind,
            ExecutionErrorKind::Invalid
        );
        store.call(&instance, "run", &[]).unwrap();
    }
    #[test]
    fn output_limit_and_trap_sources() {
        use std::error::Error;
        let cell = Cell::new().unwrap();
        let component = cell
            .component_from_bytes(
                r#"(component
            (core module $m (func (export "run") (result i32) i32.const 42))
            (core instance $i (instantiate $m))
            (func (export "run") (result u32) (canon lift (core func $i "run"))))"#,
            )
            .unwrap();
        let mut store = cell.managed_store(
            (),
            ExecutionPolicy {
                output_bytes: Some(3),
                ..Default::default()
            },
        );
        let instance = cell.instantiate_managed(&mut store, &component).unwrap();
        for _ in 0..2 {
            assert_eq!(
                store.call(&instance, "run", &[]).unwrap_err().kind,
                ExecutionErrorKind::Output
            );
        }
        let instance = cell
            .instantiate_managed(&mut store, &guest(&cell, "unreachable"))
            .unwrap();
        let error = store.call(&instance, "run", &[]).unwrap_err();
        assert_eq!(error.kind, ExecutionErrorKind::Trap);
        assert!(error.source().is_some());
    }
}
