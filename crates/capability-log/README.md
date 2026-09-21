# Explicit native and component logging

`Logger` is a cloneable native handle to an injected trusted host callback. It needs no Cell instance, Wasmtime engine, or linker to operate. Construction performs no registration and grants no component authority.

Run the native example offline:

```sh
cargo run --offline -p cell-capability-log --example native
```

```rust
use cell_capability_log::{Logger, LogError};

let logger = Logger::new(|message| {
    println!("{message}");
    Ok(())
});
logger.log("Hello from Cell")?;
# Ok::<(), LogError>(())
```

## Share the same sink with an explicitly installed adapter

```rust
use cell_capability_log::{Logger, LogCapability};
use std::sync::{Arc, Mutex};

let messages = Arc::new(Mutex::new(Vec::<String>::new()));
let sink_messages = Arc::clone(&messages);
let logger = Logger::new(move |message| {
    sink_messages.lock().unwrap().push(message.to_owned());
    Ok(())
});
logger.log("native\nmessage")?;
let cell = cell::Cell::new()?.with(LogCapability::new(logger.clone()))?;
assert_eq!(&*messages.lock().unwrap(), &["native\nmessage"]);
assert_eq!(cell.capabilities(), &["cell:log/log@0.1.0"]);
// Components instantiated in this Cell can now import the unchanged log operation.
// Their messages reach the same `messages` collection through Logger::log.
# Ok::<(), anyhow::Error>(())
```

Messages are forwarded unchanged: no prefixes, levels, timestamps, or line endings are added by `Logger` or `LogCapability`. Clones share the callback and captured state via an `Arc`; there are no guarantees of call ordering, buffering, backpressure, or transactional behavior. The mutex in the example is the sink's synchronization, not a logger guarantee.

## Failures and trust

A sink returns `Result<(), LogError>`. Construct a failure with `LogError::new("sink unavailable")`; native callers receive the typed error directly and can access its description with `message()`. There is deliberately no error classification taxonomy.

The existing WIT `cell:log/log@0.1.0` operation still accepts a string and returns no value. A sink failure propagates through Wasmtime's host-call failure path as component execution failure. The guest does **not** receive a typed `LogError`.

Sinks are trusted host code, outside the guest sandbox. They may block, fail, or cause irreversible side effects. Synchronization and resource handling remain their responsibility; Cell does not sandbox or roll back their effects.

An empty Cell has no logging interface. Only explicit installation grants access. Capability identity and descriptors remain advisory installation metadata, not authorization checks, and duplicate interface registration remains an error.

## Stdout compatibility

`StdoutLog`, `StdoutLog::new()`, and `StdoutLog::default()` remain supported with the existing `Capability<T>` implementation and interface identity. `Cell::new()?.with(StdoutLog)?` remains valid. Stdout registration delegates through the same adapter and logger; its sink uses `println!`, preserving the existing trailing newline and stdout behavior. Stdout is never installed implicitly. As before, `println!` may panic on stdout write failure; injected sinks can instead report typed failures.
