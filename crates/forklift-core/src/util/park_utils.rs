use std::path::PathBuf;
use crate::globals::bay_root;
use crate::util::file_utils;

/// The name of the parked-parcels file (inside the forklift root folder): one parcel hash
/// per line, the last line being the most recently parked one. Parked parcels are regular
/// parcel objects (their parent is the head they were parked on); this file is the only
/// thing that references them.
const FILE_NAME_PARKED: &str = "parked";

/// Get the path of the parked-parcels file (bay-local: parked work belongs to the bay).
fn get_parked_path() -> PathBuf {
    bay_root().join(FILE_NAME_PARKED)
}

/// Read the parked parcel hashes (oldest first).
///
/// # Returns
/// * `Ok(Vec<String>)` - The parked parcel hashes.
/// * `Err(String)`     - If the file exists but could not be read or holds invalid data.
pub fn read_parked() -> Result<Vec<String>, String> {
    let path = get_parked_path();

    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Error while reading \"{}\": {}", path.to_string_lossy(), e))?;

    let mut hashes: Vec<String> = Vec::new();

    for line in content.lines() {
        let is_valid = line.len() == 64 && line.bytes().all(|b| b.is_ascii_hexdigit());

        if !is_valid {
            return Err(format!(
                "The parked-parcels file \"{}\" is malformed; fix it by hand.",
                path.to_string_lossy()
            ));
        }

        hashes.push(line.to_string());
    }

    Ok(hashes)
}

/// Write the parked parcel hashes (atomically).
///
/// # Arguments
/// * `hashes` - The parked parcel hashes (oldest first). An empty list removes the file.
///
/// # Returns
/// * `Ok(())`      - If the file was written (or removed).
/// * `Err(String)` - If the file could not be written.
pub fn write_parked(hashes: &[String]) -> Result<(), String> {
    let path = get_parked_path();

    if hashes.is_empty() {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Error while removing \"{}\": {}", path.to_string_lossy(), e)),
        };
    }

    let mut content = hashes.join("\n");
    content.push('\n');

    file_utils::write_file_atomically(&path, content.as_bytes())
}
