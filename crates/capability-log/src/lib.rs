#![doc = include_str!("../README.md")]

use std::{fmt, sync::Arc};
use wasmtime::component::Linker;

/// The unchanged component logging interface.
pub const LOG_INTERFACE: &str = "cell:log/log@0.1.0";

/// A sink-reported logging failure, without a classification taxonomy.
///
/// Native callers receive this type directly. Component calls fail through
/// Wasmtime's host-call error path; guests do not receive a typed `LogError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogError {
    message: String,
}

impl LogError {
    /// Describe a failure reported by the trusted sink.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The sink's failure description.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for LogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LogError {}

type Sink = dyn Fn(&str) -> Result<(), LogError> + Send + Sync + 'static;

/// An explicitly configured, shared handle to a trusted host sink.
///
/// Clones share the same sink, not copies of its state. Messages are forwarded
/// unchanged. No ordering, buffering, backpressure, or transaction guarantees
/// are provided. The sink may block, fail, or cause irreversible side effects;
/// its synchronization and resource management are the host's responsibility.
#[derive(Clone)]
pub struct Logger {
    sink: Arc<Sink>,
}

impl Logger {
    /// Construct a logger without creating a Cell, engine, or linker, or
    /// registering any component authority.
    pub fn new(sink: impl Fn(&str) -> Result<(), LogError> + Send + Sync + 'static) -> Self {
        Self {
            sink: Arc::new(sink),
        }
    }

    /// Forward exactly this message to the configured sink.
    pub fn log(&self, message: &str) -> Result<(), LogError> {
        (self.sink)(message)
    }
}

/// Exposes a logger only when explicitly installed through Cell's capability
/// mechanism. Metadata describes the installation; it is not authorization.
pub struct LogCapability {
    logger: Logger,
}

impl LogCapability {
    pub fn new(logger: Logger) -> Self {
        Self { logger }
    }
}

impl<T: Send + 'static> cell::Capability<T> for LogCapability {
    fn name(&self) -> &'static str {
        LOG_INTERFACE
    }

    fn add_to_linker(self, linker: &mut Linker<T>) -> anyhow::Result<()> {
        linker.instance(LOG_INTERFACE)?.func_wrap(
            "log",
            move |_store, (message,): (String,)| {
                self.logger.log(&message)?;
                Ok(())
            },
        )?;
        Ok(())
    }
}

/// The opt-in stdout capability, preserving unit-struct construction, `new`,
/// and `Default`. Its trusted sink prints each message with a trailing newline,
/// as before; the shared logger itself adds no formatting.
pub struct StdoutLog;

impl StdoutLog {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StdoutLog {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + 'static> cell::Capability<T> for StdoutLog {
    fn name(&self) -> &'static str {
        LOG_INTERFACE
    }

    fn add_to_linker(self, linker: &mut Linker<T>) -> anyhow::Result<()> {
        LogCapability::new(Logger::new(|message| {
            println!("{message}");
            Ok(())
        }))
        .add_to_linker(linker)
    }
}
