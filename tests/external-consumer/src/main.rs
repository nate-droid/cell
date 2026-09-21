use cell::{Capability, Cell};
use cell_capability_fs::{LocalFs, ReadOnlyFs, Workspace};
use cell_capability_log::StdoutLog;
use cell_capability_openai::OpenAiChat;
use cell_compose::Composer;
use cell_guest::{CellPath, fs};

fn capability<C: Capability<()>>() {}

fn main() {
    capability::<LocalFs>();
    capability::<ReadOnlyFs>();
    capability::<Workspace>();
    capability::<StdoutLog>();
    capability::<OpenAiChat>();
    let _ = Composer::application(Vec::<u8>::new());
    let cell = Cell::new().unwrap().with(StdoutLog).unwrap();
    assert_eq!(cell.capabilities(), &["cell:log/log@0.1.0"]);
    assert_eq!(CellPath::new("input.txt").unwrap().as_str(), "input.txt");
    // Compile guest bindings, but never invoke guest imports in a host process.
    let _: fn(&CellPath) -> Result<fs::Metadata, String> = |path| fs::symlink_metadata(path);
}
