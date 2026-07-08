use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use crate::enums::dir_entry_type::DirEntryType;
use crate::model::inventory::Inventory;
use crate::model::tree_item::TreeItem;
use crate::util::{file_utils, inventory_utils, object_utils};

/// A single file operation needed to turn one tree into another in the working directory.
pub enum FileOp {
    /// Write the blob with the given hash to the given path.
    Write {
        /// The warehouse path of the file.
        path: String,
        /// The blob hash of the content.
        hash: String,
        /// The type of the entry (normal / executable / symlink).
        item_type: DirEntryType,
        /// Whether the file is new (it does not exist in the source tree). New files must
        /// not overwrite anything — an existing (untracked) file at the path is a conflict.
        is_new: bool,
    },

    /// Remove the file at the given path.
    Remove {
        /// The warehouse path of the file.
        path: String,
    },
}

/// Compute the file operations that turn the `from` tree into the `to` tree, and the
/// directories that exist only in the `from` tree (candidates for removal, deepest first).
///
/// Subtrees with identical hashes are skipped entirely — unchanged parts of the warehouse
/// are never visited.
///
/// # Arguments
/// * `from` - The hash of the source root tree (`None` when shifting from an unborn pallet).
/// * `to`   - The hash of the target root tree.
///
/// # Returns
/// * `Ok((Vec<FileOp>, Vec<String>))` - The file operations and the removed directories
///                                      (deepest first).
/// * `Err(String)`                    - If a tree object could not be loaded.
pub fn diff_trees(from: Option<&str>, to: &str) -> Result<(Vec<FileOp>, Vec<String>), String> {
    let from_tree = match from {
        Some(hash) => Some(object_utils::load_tree(hash)?),
        None => None,
    };
    let to_tree = object_utils::load_tree(to)?;

    let mut ops: Vec<FileOp> = Vec::new();
    let mut removed_dirs: Vec<String> = Vec::new();

    diff_directory(from_tree.as_ref(), Some(&to_tree), "", &mut ops, &mut removed_dirs)?;

    // Deepest directories first, so empty-directory cleanup can proceed bottom-up.
    removed_dirs.sort_by_key(|dir| std::cmp::Reverse(dir.matches('/').count()));

    Ok((ops, removed_dirs))
}

/// Diff one directory level of two trees (recursively).
///
/// # Arguments
/// * `from`         - This directory in the source tree (if it exists there).
/// * `to`           - This directory in the target tree (if it exists there).
/// * `key`          - The warehouse path key of the directory.
/// * `ops`          - The collected file operations.
/// * `removed_dirs` - The collected source-only directories.
///
/// # Returns
/// * `Ok(())`      - If the directory was diffed.
/// * `Err(String)` - If a subtree object could not be loaded.
fn diff_directory(from: Option<&TreeItem>,
                  to: Option<&TreeItem>,
                  key: &str,
                  ops: &mut Vec<FileOp>,
                  removed_dirs: &mut Vec<String>) -> Result<(), String> {
    let from_files: BTreeMap<&String, &TreeItem> = from
        .map(|tree| tree.get_files().collect())
        .unwrap_or_default();
    let to_files: BTreeMap<&String, &TreeItem> = to
        .map(|tree| tree.get_files().collect())
        .unwrap_or_default();

    for (name, to_item) in &to_files {
        let from_item = from_files.get(*name);

        let is_unchanged = from_item
            .map(|item| item.hash == to_item.hash && item.item_type == to_item.item_type)
            .unwrap_or(false);

        if !is_unchanged {
            ops.push(FileOp::Write {
                path: join_key(key, name),
                hash: to_item.hash.clone(),
                item_type: to_item.item_type,
                is_new: from_item.is_none(),
            });
        }
    }

    for (name, _) in &from_files {
        if !to_files.contains_key(*name) {
            ops.push(FileOp::Remove { path: join_key(key, name) });
        }
    }

    let from_subtrees: BTreeMap<&String, &TreeItem> = from
        .map(|tree| tree.get_subtrees().collect())
        .unwrap_or_default();
    let to_subtrees: BTreeMap<&String, &TreeItem> = to
        .map(|tree| tree.get_subtrees().collect())
        .unwrap_or_default();

    for (name, to_subtree) in &to_subtrees {
        let from_subtree = from_subtrees.get(*name);

        // Identical subtree hashes mean identical content all the way down.
        if from_subtree.map(|subtree| subtree.hash == to_subtree.hash).unwrap_or(false) {
            continue;
        }

        let from_loaded = match from_subtree {
            Some(subtree) => Some(object_utils::load_tree(&subtree.hash)?),
            None => None,
        };
        let to_loaded = object_utils::load_tree(&to_subtree.hash)?;

        diff_directory(
            from_loaded.as_ref(),
            Some(&to_loaded),
            &join_key(key, name),
            ops,
            removed_dirs
        )?;
    }

    for (name, from_subtree) in &from_subtrees {
        if !to_subtrees.contains_key(*name) {
            let from_loaded = object_utils::load_tree(&from_subtree.hash)?;
            let subtree_key = join_key(key, name);

            diff_directory(Some(&from_loaded), None, &subtree_key, ops, removed_dirs)?;
            removed_dirs.push(subtree_key);
        }
    }

    Ok(())
}

/// Apply a file operation to the working directory.
///
/// # Arguments
/// * `op` - The file operation to apply.
///
/// # Returns
/// * `Ok(())`      - If the operation was applied.
/// * `Err(String)` - If a file system operation failed.
pub fn apply_file_op(op: &FileOp) -> Result<(), String> {
    match op {
        FileOp::Write { path, hash, item_type, .. } => write_tracked_file(path, hash, *item_type),
        FileOp::Remove { path } => {
            match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                // The file being gone already is the desired outcome.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(format!("Error while removing \"{}\": {}", path, e)),
            }
        }
    }
}

/// Write a tracked file (normal, executable or symlink) from its blob to the working
/// directory, creating parent directories as needed.
///
/// # Arguments
/// * `path`      - The warehouse path of the file.
/// * `hash`      - The blob hash of the content.
/// * `item_type` - The type of the entry.
///
/// # Returns
/// * `Ok(())`      - If the file was written.
/// * `Err(String)` - If the blob could not be loaded or a file system operation failed.
pub fn write_tracked_file(path: &str, hash: &str, item_type: DirEntryType) -> Result<(), String> {
    let blob = object_utils::load_blob(hash)?;
    let fs_path = Path::new(path);

    if let Some(parent) = fs_path.parent() {
        if !parent.as_os_str().is_empty() {
            file_utils::create_folder_if_not_exists(parent)?;
        }
    }

    if item_type == DirEntryType::SymbolicLink {
        let target = String::from_utf8(blob.content)
            .map_err(|_| format!("The symlink target stored for \"{}\" is not valid UTF-8.", path))?;

        // A symlink cannot be overwritten in place; remove whatever is at the path first.
        match std::fs::remove_file(fs_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Error while replacing \"{}\": {}", path, e)),
        }

        #[cfg(unix)]
        return std::os::unix::fs::symlink(&target, fs_path)
            .map_err(|e| format!("Error while creating symlink \"{}\": {}", path, e));

        #[cfg(windows)]
        return Err(format!(
            "Cannot materialize symlink \"{}\" (pointing at \"{}\") on Windows.",
            path, target
        ));
    }

    std::fs::write(fs_path, &blob.content)
        .map_err(|e| format!("Error while writing \"{}\": {}", path, e))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = if item_type == DirEntryType::Executable { 0o755 } else { 0o644 };

        std::fs::set_permissions(fs_path, std::fs::Permissions::from_mode(mode))
            .map_err(|e| format!("Error while setting the permissions of \"{}\": {}", path, e))?;
    }

    Ok(())
}

/// Try to remove the given directories (deepest first). Directories that still contain
/// anything (e.g. untracked files) are silently kept.
///
/// # Arguments
/// * `removed_dirs` - The directories to remove, deepest first.
pub fn remove_empty_directories(removed_dirs: &[String]) {
    for dir in removed_dirs {
        // A failure means the directory is not empty (untracked content) or already gone;
        // both are fine.
        let _ = std::fs::remove_dir(dir);
    }
}

/// Build the inventory shards for the given tree: one shard per directory, entries built
/// from the (just materialized) files' metadata — nothing is read or hashed.
///
/// # Arguments
/// * `root_tree_hash` - The hash of the root tree to build shards for.
///
/// # Returns
/// * `Ok(BTreeMap<String, Inventory>)` - Warehouse path key → inventory.
/// * `Err(String)`                     - If a tree object could not be loaded or a file's
///                                       metadata could not be gathered.
pub fn build_inventories_for_tree(root_tree_hash: &str) -> Result<BTreeMap<String, Inventory>, String> {
    let mut shards: BTreeMap<String, Inventory> = BTreeMap::new();
    let root_tree = object_utils::load_tree(root_tree_hash)?;

    build_inventory_for_tree_directory(&root_tree, "", &mut shards)?;

    Ok(shards)
}

/// Build the inventory shard for one directory of a tree (recursively).
///
/// # Arguments
/// * `tree`   - The (loaded) tree of the directory.
/// * `key`    - The warehouse path key of the directory.
/// * `shards` - The collected shards.
///
/// # Returns
/// * `Ok(())`      - If the shard was built.
/// * `Err(String)` - If a subtree could not be loaded or a file's metadata gathered.
fn build_inventory_for_tree_directory(tree: &TreeItem,
                                      key: &str,
                                      shards: &mut BTreeMap<String, Inventory>) -> Result<(), String> {
    let mut inventory = Inventory::new();

    for (name, item) in tree.get_files() {
        let file_path = PathBuf::from(join_key(key, name));

        inventory.add_item(inventory_utils::build_inventory_item_from_stat(
            &file_path,
            name,
            item.hash.clone()
        )?);
    }

    shards.insert(key.to_string(), inventory);

    for (name, subtree) in tree.get_subtrees() {
        let subtree_loaded = object_utils::load_tree(&subtree.hash)?;
        build_inventory_for_tree_directory(&subtree_loaded, &join_key(key, name), shards)?;
    }

    Ok(())
}

/// Join a directory key and an entry name into the entry's warehouse path.
fn join_key(key: &str, name: &str) -> String {
    if key.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", key, name)
    }
}

/// Materialize a target tree over the working directory and rebuild the whole staging
/// area from it — the shared engine of every head-moving operation that arrives from a
/// remote (`lower`, `franchise`). Refuses up front (before anything is touched) when a
/// new file would overwrite untracked content.
///
/// # Arguments
/// * `from_tree`  - The tree the working directory currently matches (`None` for an
///                  empty/unborn state).
/// * `to_tree`    - The tree to materialize.
/// * `operation`  - What is running (for the collision error message).
///
/// # Returns
/// * `Ok(())`      - If the working directory and the inventory now match `to_tree`.
/// * `Err(String)` - On untracked collisions, or a failed write.
pub fn materialize_tree(from_tree: Option<&str>,
                        to_tree: &str,
                        operation: &str) -> Result<(), String> {
    let (ops, removed_dirs) = diff_trees(from_tree, to_tree)?;

    let conflicts: Vec<&String> = ops.iter()
        .filter_map(|op| match op {
            FileOp::Write { path, is_new: true, .. }
                if std::path::Path::new(path).exists() => Some(path),
            _ => None,
        })
        .collect();

    if !conflicts.is_empty() {
        return Err(format!(
            "{} would overwrite these untracked files:\n  {}\n\
            Move them out of the way (or load and stack them) first.",
            operation,
            conflicts.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")
        ));
    }

    for op in &ops {
        apply_file_op(op)?;
    }

    remove_empty_directories(&removed_dirs);

    let shards = build_inventories_for_tree(to_tree)?;

    crate::util::inventory_utils::replace_all_inventories(&shards)?;

    Ok(())
}
