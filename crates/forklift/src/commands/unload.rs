use serde::Serialize;
use forklift_core::util::inventory_utils;
use forklift_core::util::path_utils::WarehousePath;
use crate::output::{self, CommandOutput};

/// Handle the unload command.
/// This command stages a file or directory for removal: the corresponding inventory entries
/// are marked as `Deleted` (they will not be part of the next parcel). The entries are kept
/// in the inventory so the staged removal is remembered; the working directory is untouched.
///
/// # Arguments
/// * `subject` - The path of the file or directory to unload.
///
/// # Returns
/// * `Ok(())`      - If the command was handled successfully.
/// * `Err(String)` - If there was an error while handling the command.
pub fn handle_command(subject: &str) -> Result<(), String> {
    let path = WarehousePath::from_user_input(subject)?;
    inventory_utils::stage_removal(&path)?;

    output::emit("unload", &Unloaded { path: path.as_key().to_string() });

    Ok(())
}

/// The path an `unload` staged for removal. Human output stays silent; `--json` gets
/// a confirmation envelope.
#[derive(Serialize)]
struct Unloaded {
    path: String,
}

impl CommandOutput for Unloaded {
    fn render_human(&self) {}
}
