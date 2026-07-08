use serde::Serialize;
use crate::output::{self, CommandOutput};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Handle the "version" command.
///
/// # Returns
/// * `Ok(())` - The command always succeeds.
pub fn handle_command() -> Result<(), String> {
    output::emit("version", &Version { version: VERSION.to_string() });

    Ok(())
}

/// The forklift version.
#[derive(Serialize)]
struct Version {
    version: String,
}

impl CommandOutput for Version {
    fn render_human(&self) {
        println!("Forklift version {}", self.version);
    }
}
