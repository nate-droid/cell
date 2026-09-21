#![no_std]

extern crate alloc;

use alloc::{format, string::String, string::ToString, vec::Vec};

use cell_guest::{CellPath, fs};

mod bindings {
    wit_bindgen::generate!({
        world: "fs-demo",
        path: "wit",
        generate_all,
    });

    use super::GuestComponent;

    export!(GuestComponent);
}

struct GuestComponent;

impl bindings::Guest for GuestComponent {
    fn run() -> Result<String, String> {
        let input = fs::read_text(CellPath::new("input.txt").map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
        fs::write_text(
            CellPath::new("output.txt").map_err(|error| error.to_string())?,
            &format!("processed: {input}"),
        )
        .map_err(|error| error.to_string())?;
        fs::create_dir(CellPath::new("created").map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;

        let entries = fs::list_dir(CellPath::root())
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>()
            .join(",");

        Ok(format!("input={input}; entries={entries}"))
    }
}
