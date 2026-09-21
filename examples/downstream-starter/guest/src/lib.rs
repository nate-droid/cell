mod bindings {
    wit_bindgen::generate!({ world: "starter", path: "wit", generate_all });
    use super::GuestComponent;
    export!(GuestComponent);
}

struct GuestComponent;

impl bindings::Guest for GuestComponent {
    fn run() -> Result<String, String> {
        use bindings::cell::{fs::fs, log::log};
        log::log("downstream guest started");
        let input = fs::read_text("input.txt").map_err(|error| format!("read: {error:?}"))?;
        match fs::write_text("denied.txt", "must not be written") {
            Err(error) if matches!(error.category, fs::ErrorCategory::PermissionDenied) => {}
            other => return Err(format!("expected permission-denied, got {other:?}")),
        }
        Ok(format!("read={}; write=permission-denied", input.trim()))
    }
}
