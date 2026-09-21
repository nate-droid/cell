mod bindings {
    wit_bindgen::generate!({
        world: "hello",
        path: "wit",
        generate_all,
    });

    use super::GuestComponent;

    export!(GuestComponent);
}

struct GuestComponent;

impl bindings::Guest for GuestComponent {
    fn run() {
        bindings::cell::log::log::log("hello from a Cell guest");
    }
}
