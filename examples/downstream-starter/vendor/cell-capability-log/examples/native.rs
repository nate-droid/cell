use cell_capability_log::Logger;

fn main() -> Result<(), cell_capability_log::LogError> {
    let logger = Logger::new(|message| {
        println!("{message}");
        Ok(())
    });
    logger.log("Hello from Cell")
}
