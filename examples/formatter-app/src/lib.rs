mod bindings {
    wit_bindgen::generate!({
        world: "formatter-app",
        path: "wit",
        generate_all,
    });

    use super::GuestComponent;

    export!(GuestComponent);
}

struct GuestComponent;

impl bindings::Guest for GuestComponent {
    fn run(input: String) -> String {
        bindings::cell_example::formatter::formatter::format(&input)
    }
}
