//! Additive execution metadata, not a replay guarantee or an authority attestation.
use crate::{ArtifactDigest, Cell, ExecutionError, ExecutionErrorKind, ExecutionPolicy};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant, SystemTime};
use wasmtime::component::Val;

/// Numeric limits are separate from wall-clock and cancellation observations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedLimits {
    pub fuel: Option<u64>,
    pub memory_bytes: Option<usize>,
    pub input_bytes: Option<usize>,
    pub output_bytes: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionInputs {
    /// Identity of the exact supplied bytes, including WAT if supplied.
    pub component_digest: ArtifactDigest,
    pub export: String,
    /// Unique installation-order identifiers from `Cell::with` only. Direct
    /// linker registrations, authority configuration, and host state are untracked.
    pub capabilities: Vec<&'static str>,
    pub limits: RecordedLimits,
    pub input_digest: Option<ArtifactDigest>,
}

#[derive(Clone, Debug)]
pub struct ExecutionObservations {
    pub started_at: SystemTime,
    pub elapsed: Duration,
    pub deadline: Option<Instant>,
    pub cancellation_present: bool,
    pub cancelled_at_start: bool,
    pub cancelled_at_end: bool,
    /// None means the phase never received a fuel budget; Some(0) is measured zero.
    pub startup_fuel: Option<u64>,
    /// Includes canonical post-return and failed calls; separate because fuel resets.
    pub call_fuel: Option<u64>,
}

/// No payloads, host state, or error messages are retained here.
#[derive(Clone, Debug)]
pub struct ExecutionRecord {
    pub inputs: ExecutionInputs,
    pub observations: ExecutionObservations,
    /// None denotes success, including an empty result vector.
    pub error_kind: Option<ExecutionErrorKind>,
    pub output_digest: Option<ArtifactDigest>,
}

#[derive(Debug)]
pub struct RecordedExecution {
    pub result: Result<Vec<Val>, ExecutionError>,
    pub record: ExecutionRecord,
}

impl<T: 'static> Cell<T> {
    /// Compile the exact supplied bytes and execute in a fresh managed store.
    /// Returns metadata even on compilation, linking, or execution failure.
    /// Timing includes hashing/compilation; policy controls guest execution, not
    /// compilation or hashing. Host side effects are not rolled back. Digests
    /// do not capture host state, authority, engine configuration, or external I/O.
    pub fn execute_recorded(
        &self,
        bytes: impl AsRef<[u8]>,
        state: T,
        policy: ExecutionPolicy,
        export: &str,
        params: &[Val],
    ) -> RecordedExecution {
        let started_at = SystemTime::now();
        let start = Instant::now();
        let bytes = bytes.as_ref();
        let cancelled = || {
            policy
                .cancellation
                .as_ref()
                .is_some_and(|t| t.is_cancelled())
        };
        let cancelled_at_start = cancelled();
        let inputs = ExecutionInputs {
            component_digest: ArtifactDigest::of(bytes),
            export: export.to_owned(),
            capabilities: self.capabilities().to_vec(),
            limits: RecordedLimits {
                fuel: policy.fuel,
                memory_bytes: policy.memory_bytes,
                input_bytes: policy.input_bytes,
                output_bytes: policy.output_bytes,
            },
            input_digest: values_digest(params),
        };
        let mut startup_fuel = None;
        let mut call_fuel = None;
        let result = self
            .component_from_bytes(bytes)
            .map_err(ExecutionError::classify)
            .and_then(|component| {
                let mut store = self.managed_store(state, policy.clone());
                let instance = self.instantiate_managed(&mut store, &component);
                startup_fuel = store.measured_fuel();
                let instance = instance?;
                store.clear_fuel_measurement();
                let result = store.call(&instance, export, params);
                call_fuel = store.measured_fuel();
                result
            });
        let record = ExecutionRecord {
            inputs,
            output_digest: result.as_ref().ok().and_then(|v| values_digest(v)),
            error_kind: result.as_ref().err().map(|e| e.kind),
            observations: ExecutionObservations {
                started_at,
                elapsed: start.elapsed(),
                deadline: policy.deadline,
                cancellation_present: policy.cancellation.is_some(),
                cancelled_at_start,
                cancelled_at_end: cancelled(),
                startup_fuel,
                call_fuel,
            },
        };
        RecordedExecution { result, record }
    }
}

/// SHA-256 of a versioned, length-delimited, type-tagged encoding. Order is
/// preserved for all containers (including records, flags, and maps). Floats
/// hash their exact IEEE bits, distinguishing signed zero and NaN payloads.
/// Resources and unsupported values anywhere in the tree return None.
/// This hashes dynamic values, not their WIT types or canonical ABI bytes.
pub fn values_digest(values: &[Val]) -> Option<ArtifactDigest> {
    fn length(h: &mut Sha256, n: usize) {
        h.update((n as u64).to_le_bytes());
    }
    fn text(h: &mut Sha256, s: &str) {
        length(h, s.len());
        h.update(s.as_bytes());
    }
    enum Item<'a> {
        Value(&'a Val),
        Text(&'a str),
    }
    let mut h = Sha256::new();
    h.update(b"cell-values-v1\0");
    length(&mut h, values.len());
    let mut pending: Vec<_> = values.iter().rev().map(Item::Value).collect();
    while let Some(item) = pending.pop() {
        let v = match item {
            Item::Text(s) => {
                text(&mut h, s);
                continue;
            }
            Item::Value(v) => v,
        };
        macro_rules! scalar {
            ($tag:expr, $bytes:expr) => {{
                h.update([$tag]);
                h.update($bytes);
            }};
        }
        match v {
            Val::Bool(v) => scalar!(0, [u8::from(*v)]),
            Val::S8(v) => scalar!(1, v.to_le_bytes()),
            Val::U8(v) => scalar!(2, v.to_le_bytes()),
            Val::S16(v) => scalar!(3, v.to_le_bytes()),
            Val::U16(v) => scalar!(4, v.to_le_bytes()),
            Val::S32(v) => scalar!(5, v.to_le_bytes()),
            Val::U32(v) => scalar!(6, v.to_le_bytes()),
            Val::S64(v) => scalar!(7, v.to_le_bytes()),
            Val::U64(v) => scalar!(8, v.to_le_bytes()),
            Val::Float32(v) => scalar!(9, v.to_bits().to_le_bytes()),
            Val::Float64(v) => scalar!(10, v.to_bits().to_le_bytes()),
            Val::Char(v) => scalar!(11, (*v as u32).to_le_bytes()),
            Val::String(v) => {
                h.update([12]);
                text(&mut h, v);
            }
            Val::List(items) | Val::Tuple(items) => {
                h.update([if matches!(v, Val::List(_)) { 13 } else { 14 }]);
                length(&mut h, items.len());
                pending.extend(items.iter().rev().map(Item::Value));
            }
            Val::Record(fields) => {
                h.update([15]);
                length(&mut h, fields.len());
                for (name, value) in fields.iter().rev() {
                    pending.push(Item::Value(value));
                    pending.push(Item::Text(name));
                }
            }
            Val::Variant(name, value) => {
                h.update([16]);
                text(&mut h, name);
                h.update([u8::from(value.is_some())]);
                pending.extend(value.as_deref().map(Item::Value));
            }
            Val::Enum(name) => {
                h.update([17]);
                text(&mut h, name);
            }
            Val::Option(value) => {
                h.update([18, u8::from(value.is_some())]);
                pending.extend(value.as_deref().map(Item::Value));
            }
            Val::Result(value) => {
                h.update([19, u8::from(value.is_ok())]);
                let value = match value {
                    Ok(v) | Err(v) => v,
                };
                h.update([u8::from(value.is_some())]);
                pending.extend(value.as_deref().map(Item::Value));
            }
            Val::Flags(names) => {
                h.update([20]);
                length(&mut h, names.len());
                for name in names {
                    text(&mut h, name);
                }
            }
            Val::Map(entries) => {
                h.update([21]);
                length(&mut h, entries.len());
                for (k, v) in entries.iter().rev() {
                    pending.push(Item::Value(v));
                    pending.push(Item::Value(k));
                }
            }
            _ => return None,
        }
    }
    // ArtifactDigest uses the same SHA-256 representation and strict parser.
    let hex = format!("sha256:{:x}", h.finalize());
    hex.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CancellationToken, Capability};
    fn guest(body: &str) -> String {
        format!(
            r#"(component (core module $m (func (export "run") {body}))
            (core instance $i (instantiate $m))
            (func (export "run") (canon lift (core func $i "run"))))"#
        )
    }
    fn run(cell: &Cell, bytes: &str, policy: ExecutionPolicy) -> RecordedExecution {
        cell.execute_recorded(bytes, (), policy, "run", &[])
    }
    #[test]
    fn success_identity_limits_and_recovery() {
        struct Named;
        impl Capability<()> for Named {
            fn name(&self) -> &'static str {
                "test:identity"
            }
            fn add_to_linker(self, _: &mut wasmtime::component::Linker<()>) -> anyhow::Result<()> {
                Ok(())
            }
        }
        let cell = Cell::new().unwrap().with(Named).unwrap();
        let bytes = guest("");
        let policy = ExecutionPolicy {
            fuel: Some(100),
            ..Default::default()
        };
        let a = run(&cell, &bytes, policy.clone());
        let b = run(&cell, &bytes, policy);
        assert!(a.result.unwrap().is_empty());
        assert_eq!(a.record.inputs, b.record.inputs);
        assert_eq!(
            a.record.inputs.component_digest,
            ArtifactDigest::of(bytes.as_bytes())
        );
        assert_eq!(a.record.inputs.capabilities, ["test:identity"]);
        assert_eq!(a.record.output_digest, values_digest(&[]));
        assert!(a.record.observations.startup_fuel.is_some());
        assert!(a.record.observations.call_fuel.unwrap() > 0);
        for _ in 0..3 {
            let failed = run(&cell, &guest("unreachable"), ExecutionPolicy::default());
            assert_eq!(failed.record.error_kind, Some(ExecutionErrorKind::Trap));
            assert!(failed.record.observations.call_fuel.unwrap() > 0);
            assert!(failed.record.output_digest.is_none());
            assert!(
                run(&cell, &bytes, ExecutionPolicy::default())
                    .result
                    .is_ok()
            );
        }
    }
    #[test]
    fn failures_always_have_records_and_phase_fuel() {
        let mut cell = Cell::new().unwrap();
        let malformed = run(&cell, "not wasm", ExecutionPolicy::default());
        assert_eq!(
            malformed.record.error_kind,
            Some(ExecutionErrorKind::Invalid)
        );
        assert_eq!(malformed.record.observations.startup_fuel, None);
        let imports = r#"(component (import "host" (func (param "x" u32))))"#;
        let missing = run(&cell, imports, ExecutionPolicy::default());
        assert_eq!(missing.record.error_kind, Some(ExecutionErrorKind::Invalid));
        assert_eq!(missing.record.observations.startup_fuel, Some(0));
        cell.linker_mut()
            .root()
            .func_wrap("host", |_, (): ()| Ok(()))
            .unwrap();
        let incompatible = run(&cell, imports, ExecutionPolicy::default());
        assert_eq!(
            incompatible.record.error_kind,
            Some(ExecutionErrorKind::Invalid)
        );
        assert!(incompatible.record.inputs.capabilities.is_empty());
        let token = CancellationToken::default();
        token.cancel();
        let cancelled = run(
            &cell,
            &guest(""),
            ExecutionPolicy {
                cancellation: Some(token),
                ..Default::default()
            },
        );
        assert_eq!(
            cancelled.record.error_kind,
            Some(ExecutionErrorKind::Cancelled)
        );
        assert!(cancelled.record.observations.cancelled_at_start);
        assert!(cancelled.record.observations.cancelled_at_end);
        assert_eq!(cancelled.record.observations.startup_fuel, None);
        let exhausted = run(
            &cell,
            &guest("(loop br 0)"),
            ExecutionPolicy {
                fuel: Some(20),
                ..Default::default()
            },
        );
        assert_eq!(exhausted.record.error_kind, Some(ExecutionErrorKind::Fuel));
        assert_eq!(exhausted.record.observations.call_fuel, Some(20));
        let memory = run(
            &cell,
            r#"(component (core module $m (memory 1)) (core instance (instantiate $m)))"#,
            ExecutionPolicy {
                memory_bytes: Some(0),
                ..Default::default()
            },
        );
        assert_eq!(memory.record.error_kind, Some(ExecutionErrorKind::Memory));
        assert!(memory.record.observations.startup_fuel.is_some());
        assert_eq!(memory.record.observations.call_fuel, None);
        let invalid =
            cell.execute_recorded(guest(""), (), ExecutionPolicy::default(), "absent", &[]);
        assert_eq!(invalid.record.error_kind, Some(ExecutionErrorKind::Invalid));
        assert_eq!(invalid.record.observations.call_fuel, None);
    }
    #[test]
    fn deterministic_tagged_and_delimited_values() {
        let values = vec![
            Val::Record(vec![("x".into(), Val::List(vec![Val::U32(42)]))]),
            Val::Variant("case".into(), Some(Box::new(Val::Option(None)))),
        ];
        assert_eq!(values_digest(&values), values_digest(&values));
        let pairs = [
            (Val::U32(1), Val::S32(1)),
            (Val::List(vec![]), Val::Tuple(vec![])),
            (Val::String("a".into()), Val::Enum("a".into())),
            (Val::Float32(0.0), Val::Float32(-0.0)),
            (
                Val::Option(None),
                Val::Option(Some(Box::new(Val::Tuple(vec![])))),
            ),
            (Val::Result(Ok(None)), Val::Result(Err(None))),
            (
                Val::Flags(vec!["ab".into(), "c".into()]),
                Val::Flags(vec!["a".into(), "bc".into()]),
            ),
        ];
        for (a, b) in pairs {
            assert_ne!(values_digest(&[a]), values_digest(&[b]));
        }
        assert_ne!(
            values_digest(&[Val::String("ab".into()), Val::String("c".into())]),
            values_digest(&[Val::String("a".into()), Val::String("bc".into())])
        );
        let resource = Val::Resource(
            wasmtime::component::ResourceAny::try_from_resource(
                wasmtime::component::Resource::<u32>::new_own(1),
                &mut Cell::new().unwrap().store(()),
            )
            .unwrap(),
        );
        assert_eq!(values_digest(&[resource.clone()]), None);
        assert_eq!(values_digest(&[Val::List(vec![resource])]), None);
    }
}
