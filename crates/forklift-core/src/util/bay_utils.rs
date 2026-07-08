//! Bays: parallel working directories bound to one warehouse (§7.5).
//!
//! A bay is an additional working directory that **shares** the warehouse's object store,
//! refs (pallets/meta), trust and configuration, while keeping its **own** working
//! directory, inventory, current pallet and lock. N agents work one machine without
//! cloning N object stores or fighting one lock — git's worktrees, designed in rather than
//! bolted on.
//!
//! A bay's working directory holds a `.forklift` **file** (not a folder) — a redirect back
//! to the warehouse — so discovery recognizes a bay the same way it recognizes a warehouse
//! (both are `dir.join(".forklift")`, one a folder, one a file). The bay's local state
//! lives under the warehouse at `.forklift/bays/<name>/`.

use std::path::{Path, PathBuf};
use crate::globals::{bay_root, forklift_root, FOLDER_NAME_BAYS_ROOT, FOLDER_NAME_FORKLIFT_ROOT};
use crate::util::file_utils;

/// The first line of a bay's `.forklift` redirect file — how discovery tells a bay's
/// redirect from an accidental file named `.forklift`.
const BAY_REDIRECT_MAGIC: &str = "forklift-bay";

/// The file inside a bay's local state recording its working directory (so the main tree
/// can list where each bay lives).
const FILE_NAME_BAY_PATH: &str = "path";

/// A parsed bay `.forklift` redirect: the warehouse it belongs to, and the bay's name.
pub struct BayRedirect {
    /// The warehouse root (the folder containing the shared `.forklift`).
    pub warehouse_root: PathBuf,

    /// The bay's name.
    pub name: String,
}

/// Whether the `.forklift` at `path` is a bay redirect (a file) rather than a warehouse
/// (a folder).
pub fn is_bay_redirect(forklift_path: &Path) -> bool {
    forklift_path.is_file()
}

/// Write a bay's `.forklift` redirect file into its working directory.
///
/// # Arguments
/// * `bay_dir`        - The bay's working directory.
/// * `warehouse_root` - The warehouse root the bay shares.
/// * `name`           - The bay's name.
pub fn write_bay_redirect(bay_dir: &Path, warehouse_root: &Path, name: &str) -> Result<(), String> {
    let content = format!(
        "{}\n{}\n{}\n",
        BAY_REDIRECT_MAGIC, warehouse_root.to_string_lossy(), name
    );

    file_utils::write_file_atomically(&bay_dir.join(FOLDER_NAME_FORKLIFT_ROOT), content.as_bytes())
}

/// Read and validate a bay's `.forklift` redirect file.
///
/// # Arguments
/// * `forklift_path` - The path of the bay's `.forklift` file.
///
/// # Returns
/// * `Ok(BayRedirect)` - The warehouse root and bay name.
/// * `Err(String)`     - If the file is not a valid bay redirect.
pub fn read_bay_redirect(forklift_path: &Path) -> Result<BayRedirect, String> {
    let content = std::fs::read_to_string(forklift_path)
        .map_err(|e| format!("Error while reading the bay redirect \"{}\": {}", forklift_path.to_string_lossy(), e))?;

    let mut lines = content.lines();

    if lines.next() != Some(BAY_REDIRECT_MAGIC) {
        return Err(format!(
            "\"{}\" is not a valid forklift bay (its \".forklift\" file is not a bay redirect).",
            forklift_path.to_string_lossy()
        ));
    }

    let warehouse_root = lines.next()
        .filter(|line| !line.is_empty())
        .ok_or("The bay redirect has no warehouse path.".to_string())?;
    let name = lines.next()
        .filter(|line| !line.is_empty())
        .ok_or("The bay redirect has no bay name.".to_string())?;

    Ok(BayRedirect {
        warehouse_root: PathBuf::from(warehouse_root),
        name: name.to_string(),
    })
}

/// The local-state folder of the given bay (under the shared forklift root).
pub fn bay_state_dir(name: &str) -> PathBuf {
    forklift_root().join(FOLDER_NAME_BAYS_ROOT).join(name)
}

/// Whether a bay of the given name exists (its state folder is present).
pub fn does_bay_exist(name: &str) -> bool {
    bay_state_dir(name).is_dir()
}

/// Record a bay's working directory in its local state (so it can be listed).
pub fn write_bay_path(name: &str, bay_dir: &Path) -> Result<(), String> {
    let state = bay_state_dir(name);
    file_utils::create_folder_if_not_exists(&state)?;
    file_utils::write_file_atomically(&state.join(FILE_NAME_BAY_PATH), bay_dir.to_string_lossy().as_bytes())
}

/// Read a bay's recorded working directory.
pub fn read_bay_path(name: &str) -> Result<PathBuf, String> {
    let path = bay_state_dir(name).join(FILE_NAME_BAY_PATH);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Error while reading the bay path \"{}\": {}", path.to_string_lossy(), e))?;

    Ok(PathBuf::from(content.trim_end_matches('\n')))
}

/// List the names of all bays (the subfolders of `.forklift/bays/`), sorted.
///
/// # Returns
/// * `Ok(Vec<String>)` - The bay names (empty when there are none).
/// * `Err(String)`     - If the bays folder could not be read.
pub fn list_bays() -> Result<Vec<String>, String> {
    let folder = forklift_root().join(FOLDER_NAME_BAYS_ROOT);

    if !folder.is_dir() {
        return Ok(Vec::new());
    }

    let mut names: Vec<String> = Vec::new();

    for entry in file_utils::read_directory(&folder)? {
        let entry = entry.map_err(|e| format!("Error while reading a bay entry: {}", e))?;

        if file_utils::get_symlink_metadata_for_path(&entry.path())?.is_dir() {
            names.push(file_utils::get_name_for_file_or_directory(&entry)?);
        }
    }

    names.sort();

    Ok(names)
}

/// Remove a bay's local state folder. The bay's pallet (a normal ref) is left untouched;
/// removing the working directory is the caller's choice.
pub fn remove_bay_state(name: &str) -> Result<(), String> {
    let state = bay_state_dir(name);

    std::fs::remove_dir_all(&state)
        .map_err(|e| format!("Error while removing the bay state \"{}\": {}", state.to_string_lossy(), e))
}

/// Read a bay's current pallet (its bay-local `pallet` file), for listing from the main
/// tree. `None` when the bay is unborn or unreadable.
pub fn read_bay_current_pallet(name: &str) -> Option<String> {
    std::fs::read_to_string(bay_state_dir(name).join("pallet"))
        .ok()
        .map(|content| content.trim_end_matches('\n').to_string())
        .filter(|pallet| !pallet.is_empty())
}

/// The current bay's local-state folder — a convenience over [`bay_root`].
pub fn current_bay_state() -> PathBuf {
    bay_root()
}
