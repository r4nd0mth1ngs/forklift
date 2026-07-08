use serde::Serialize;
use forklift_core::util::inventory_utils;
use forklift_core::util::path_utils::WarehousePath;
use crate::output::{self, CommandOutput};

/// Handle the load command.
/// This command adds the given file or directory to its corresponding inventory.
///
/// # Arguments
/// * `target` - The path of the file or directory to load.
///
/// # Returns
/// * `Ok(())`      - If the command was handled successfully.
/// * `Err(String)` - If there was an error while handling the command.
pub async fn handle_command(target: &str) -> Result<(), String> {
    let path = WarehousePath::from_user_input(target)?;
    inventory_utils::add_changes_to_inventory(&path).await?;

    output::emit("load", &Loaded { path: path.as_key().to_string() });

    Ok(())
}

/// The path a `load` staged. Human output stays silent (as it always has); `--json`
/// still gets a confirmation envelope so a program sees a result.
#[derive(Serialize)]
struct Loaded {
    path: String,
}

impl CommandOutput for Loaded {
    fn render_human(&self) {}
}
