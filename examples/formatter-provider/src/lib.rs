mod bindings {
    wit_bindgen::generate!({
        world: "formatter-provider",
        path: "wit",
        generate_all,
    });

    use super::GuestComponent;

    export!(GuestComponent);
}

struct GuestComponent;

impl bindings::exports::cell_example::formatter::formatter::Guest for GuestComponent {
    fn format(input: String) -> String {
        format!("formatted: {input}")
    }
}
