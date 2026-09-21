use cell::{AuthorityDescription, Capability, Cell};
use cell_capability_log::{LOG_INTERFACE, LogCapability, LogError, Logger, StdoutLog};
use std::sync::{Arc, Mutex};

const LOG_COMPONENT: &str = r#"
(component
    (import "cell:log/log@0.1.0" (instance $logging
        (export "log" (func (param "message" string)))))
    (alias export $logging "log" (func $log))
    (core module $storage
        (memory (export "memory") 1)
        ;; Each test call uses a fresh instance and allocates only its input string.
        (func (export "realloc") (param i32 i32 i32 i32) (result i32)
            i32.const 8))
    (core instance $storage-instance (instantiate $storage))
    (alias core export $storage-instance "memory" (core memory $memory))
    (alias core export $storage-instance "realloc" (core func $realloc))
    (core func $lowered-log (canon lower (func $log) (memory $memory)))
    (core module $wrapper
        (import "host" "log" (func $log (param i32 i32)))
        (func (export "log") (param i32 i32)
            local.get 0
            local.get 1
            call $log))
    (core instance $host
        (export "log" (func $lowered-log)))
    (core instance $wrapper-instance
        (instantiate $wrapper (with "host" (instance $host))))
    (func (export "log") (param "message" string)
        (canon lift (core func $wrapper-instance "log")
            (memory $memory) (realloc $realloc))))
"#;

fn recording_logger() -> (Logger, Arc<Mutex<Vec<String>>>) {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&messages);
    let logger = Logger::new(move |message| {
        captured.lock().unwrap().push(message.to_owned());
        Ok(())
    });
    (logger, messages)
}

fn component_log(cell: &Cell, message: &str) -> anyhow::Result<()> {
    let component = cell.component_from_bytes(LOG_COMPONENT)?;
    let mut store = cell.store(());
    let instance = cell.instantiate(&mut store, &component)?;
    let log = instance.get_typed_func::<(String,), ()>(&mut store, "log")?;
    log.call(&mut store, (message.to_owned(),))?;

    Ok(())
}

#[test]
fn native_messages_are_unchanged_and_clones_share_the_sink() {
    let (logger, messages) = recording_logger();
    let cloned = logger.clone();
    let expected = ["", "hello", "  whitespace\n\t", "héllo 🦀", "embedded\0nul"];
    for (index, message) in expected.iter().enumerate() {
        if index % 2 == 0 {
            logger.log(message).unwrap();
        } else {
            cloned.log(message).unwrap();
        }
    }
    assert_eq!(*messages.lock().unwrap(), expected);
}

#[test]
fn logger_is_clone_send_and_sync_and_can_log_from_another_thread() {
    fn assert_traits<T: Clone + Send + Sync + 'static>() {}
    assert_traits::<Logger>();
    let (logger, messages) = recording_logger();
    let cloned = logger.clone();
    std::thread::spawn(move || cloned.log("thread message").unwrap())
        .join()
        .unwrap();
    logger.log("original message").unwrap();
    assert_eq!(
        *messages.lock().unwrap(),
        ["thread message", "original message"]
    );
}

#[test]
fn native_sink_failures_remain_typed_errors() {
    fn assert_error<T: std::error::Error + Send + Sync + 'static>() {}
    assert_error::<LogError>();
    assert_eq!(
        LogError::new("borrowed message").to_string(),
        "borrowed message"
    );
    let logger = Logger::new(|message| Err(LogError::new(format!("sink rejected: {message}"))));
    let error: LogError = logger.log("native").unwrap_err();
    assert_eq!(error.to_string(), "sink rejected: native");
    let boxed: Box<dyn std::error::Error> = Box::new(error);
    assert!(boxed.downcast_ref::<LogError>().is_some());
}

#[test]
fn component_import_calls_the_same_sink_as_native_logging() -> anyhow::Result<()> {
    let (logger, messages) = recording_logger();
    let cell = Cell::new()?.with(LogCapability::new(logger.clone()))?;
    logger.log("native before")?;
    for message in ["", "component message", "  héllo 🦀\n\t\0"] {
        component_log(&cell, message)?;
    }
    logger.log("native after")?;
    assert_eq!(
        *messages.lock().unwrap(),
        [
            "native before",
            "",
            "component message",
            "  héllo 🦀\n\t\0",
            "native after"
        ]
    );
    Ok(())
}

#[test]
fn component_sink_failure_is_a_host_error_with_log_error_in_its_chain() -> anyhow::Result<()> {
    let (logger, messages) = recording_logger();
    let failing = Logger::new(move |message| {
        logger.log(message)?;
        Err(LogError::new("sink unavailable"))
    });
    let cell = Cell::new()?.with(LogCapability::new(failing))?;
    let error = component_log(&cell, "failed component message").unwrap_err();
    assert_eq!(*messages.lock().unwrap(), ["failed component message"]);
    let typed = error
        .downcast_ref::<LogError>()
        .expect("host error retains LogError");
    assert_eq!(typed.to_string(), "sink unavailable");
    assert!(
        error
            .chain()
            .any(|cause| cause.downcast_ref::<LogError>().is_some())
    );
    assert!(error.downcast_ref::<wasmtime::Trap>().is_none());
    Ok(())
}

#[test]
fn empty_cell_does_not_supply_the_log_import() -> anyhow::Result<()> {
    let cell = Cell::<()>::empty()?;
    assert!(cell.capabilities().is_empty());
    let component = cell.component_from_bytes(LOG_COMPONENT)?;
    assert!(cell.check(&component).is_err());
    let error = cell
        .instantiate(&mut cell.store(()), &component)
        .unwrap_err();
    assert!(format!("{error:#}").contains(LOG_INTERFACE));
    Ok(())
}

#[test]
fn stdout_and_adapter_have_identical_metadata() -> anyhow::Result<()> {
    assert_eq!(LOG_INTERFACE, "cell:log/log@0.1.0");
    let _unit = StdoutLog;
    let stdout = Cell::new()?.with(StdoutLog::new())?;
    let default_stdout = Cell::new()?.with(StdoutLog::default())?;
    let (logger, _) = recording_logger();
    let adapter = Cell::new()?.with(LogCapability::new(logger))?;
    assert_eq!(stdout.capabilities(), &[LOG_INTERFACE]);
    assert_eq!(adapter.capabilities(), stdout.capabilities());
    assert_eq!(
        adapter.capability_descriptors(),
        stdout.capability_descriptors()
    );
    assert_eq!(
        default_stdout.capability_descriptors(),
        stdout.capability_descriptors()
    );
    assert_eq!(adapter.capability_descriptors().len(), 1);
    assert_eq!(adapter.capability_descriptors()[0].interface, LOG_INTERFACE);
    assert_eq!(
        adapter.capability_descriptors()[0].authority,
        AuthorityDescription::Unspecified
    );
    component_log(&stdout, "stdout component message")?;
    Ok(())
}

#[test]
fn duplicate_registrations_fail_for_both_adapters_and_cross_registration() -> anyhow::Result<()> {
    for first_stdout in [false, true] {
        for second_stdout in [false, true] {
            let (logger, messages) = recording_logger();
            let mut cell = Cell::new()?;
            if first_stdout {
                StdoutLog::new().add_to_linker(cell.linker_mut())?;
            } else {
                LogCapability::new(logger.clone()).add_to_linker(cell.linker_mut())?;
            }
            let duplicate = if second_stdout {
                StdoutLog::new().add_to_linker(cell.linker_mut())
            } else {
                LogCapability::new(logger.clone()).add_to_linker(cell.linker_mut())
            };
            assert!(
                duplicate.is_err(),
                "first stdout={first_stdout}, second stdout={second_stdout}"
            );
            component_log(&cell, "original registration")?;
            let expected: Vec<String> = if first_stdout {
                vec![]
            } else {
                vec!["original registration".to_owned()]
            };
            assert_eq!(*messages.lock().unwrap(), expected);

            let cell = Cell::new()?;
            let cell = if first_stdout {
                cell.with(StdoutLog::new())?
            } else {
                cell.with(LogCapability::new(logger.clone()))?
            };
            let duplicate = if second_stdout {
                cell.with(StdoutLog::new())
            } else {
                cell.with(LogCapability::new(logger))
            };
            assert!(duplicate.is_err());
        }
    }
    Ok(())
}
