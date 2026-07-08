use std::collections::BTreeMap;
use forklift_core::enums::dir_entry_type::DirEntryType;
use forklift_core::enums::inventory_item_state::InventoryItemState;
use forklift_core::model::inventory::Inventory;
use forklift_core::model::tree_item::TreeItem;
use forklift_core::util::path_utils::WarehousePath;
use forklift_core::util::{file_utils, inventory_utils, object_utils, pallet_utils, shift_utils};
use crate::output;

/// A resolved entry of the pallet head's tree.
enum HeadEntry {
    /// A file (normal, executable or symlink) with its blob hash.
    File { hash: String, item_type: DirEntryType },

    /// A directory, loaded.
    Tree(TreeItem),
}

/// Handle the restore command.
/// * `restore <path>`          - Rewrite the file (or every tracked file of the directory)
///                               in the working directory from the inventory, discarding
///                               unstaged changes.
/// * `restore --staged <path>` - Reset the inventory entries of the path to the pallet
///                               head (unstage), leaving the working directory untouched.
///
/// # Arguments
/// * `staged` - Whether to reset the inventory entries to the pallet head (unstage)
///              instead of restoring the working directory from the inventory.
/// * `target` - The path of the file or directory to restore.
///
/// # Returns
/// * `Ok(())`      - If the restore completed successfully.
/// * `Err(String)` - If there was an error while handling the command.
pub fn handle_command(staged: bool, target: &str) -> Result<(), String> {
    let path = WarehousePath::from_user_input(target)?;

    if staged {
        restore_staged(&path)
    } else {
        restore_worktree(&path)
    }
}

/// Restore the working directory from the inventory: rewrite the file (or, for a
/// directory, every tracked, non-`Deleted` file below it) from its staged blob.
///
/// # Arguments
/// * `path` - The path to restore.
///
/// # Returns
/// * `Ok(())`      - If the restore completed.
/// * `Err(String)` - If the path is not in the inventory or a write failed.
fn restore_worktree(path: &WarehousePath) -> Result<(), String> {
    let has_shard = file_utils::get_inventory_data_path_for_key(path.as_key()).exists();

    if path.is_root() || has_shard {
        return restore_worktree_directory(path.as_key());
    }

    let (parent, file_name) = path.split_parent()?;

    let (_, shard_bytes) = file_utils::retrieve_inventory_or_none_by_key(parent.as_key())?;
    let inventory = match shard_bytes {
        Some(bytes) => forklift_core::parser::inventory::inventory_parser::parse_inventory(&bytes)?,
        None => return Err(format!("\"{}\" is not in the inventory.", path.as_key())),
    };

    let Some(item) = inventory.get_item_by_name(&file_name) else {
        return Err(format!("\"{}\" is not in the inventory.", path.as_key()));
    };

    if item.state == InventoryItemState::Deleted {
        return Err(format!(
            "The removal of \"{}\" is staged; use \"restore --staged {}\" to unstage it first.",
            path.as_key(),
            path.as_key()
        ));
    }

    restore_file_and_refresh_entry(parent.as_key(), &item.name, &item.hash, item.item_type)?;

    output::message("restore", format!("Restored \"{}\" from the inventory.", path.as_key()));

    Ok(())
}

/// Restore every tracked file of a directory (and its subdirectories) from the inventory.
///
/// # Arguments
/// * `key` - The warehouse path key of the directory.
///
/// # Returns
/// * `Ok(())`      - If the restore completed.
/// * `Err(String)` - If the directory has no inventory or a write failed.
fn restore_worktree_directory(key: &str) -> Result<(), String> {
    let (_, metadata_opt) = file_utils::retrieve_inventory_metadata_or_none()?;
    let metadata = metadata_opt.unwrap_or_default();

    let prefix = if key.is_empty() { String::new() } else { format!("{}/", key) };
    let mut restored_any = false;
    let mut restored_count = 0usize;

    for entry in &metadata {
        let shard_key = inventory_utils::metadata_entry_to_key(entry);

        let is_in_subtree = key.is_empty()
            || shard_key == key
            || shard_key.starts_with(&prefix);

        if !is_in_subtree {
            continue;
        }

        restored_any = true;

        let (_, shard_bytes) = file_utils::retrieve_inventory_or_none_by_key(shard_key)?;

        let Some(bytes) = shard_bytes else {
            continue;
        };

        let inventory = forklift_core::parser::inventory::inventory_parser::parse_inventory(&bytes)
            .map_err(|e| format!("Error while parsing the inventory of folder \"{}\": {}", shard_key, e))?;

        for (name, item) in inventory.get_items() {
            if item.state == InventoryItemState::Deleted {
                continue;
            }

            restore_file_and_refresh_entry(shard_key, name, &item.hash, item.item_type)?;
            restored_count += 1;
        }
    }

    if !restored_any {
        return Err(format!(
            "No inventory found for folder \"{}\".",
            if key.is_empty() { "./" } else { key }
        ));
    }

    output::message("restore", format!("Restored {} file(s) from the inventory.", restored_count));

    Ok(())
}

/// Write one tracked file from its blob and refresh its inventory entry with the new
/// stat data (so the warehouse reports clean afterwards without rehashing).
///
/// # Arguments
/// * `parent_key` - The warehouse path key of the file's directory.
/// * `name`       - The name of the file.
/// * `hash`       - The blob hash of the staged content.
/// * `item_type`  - The type of the entry.
///
/// # Returns
/// * `Ok(())`      - If the file was restored.
/// * `Err(String)` - If a write failed.
fn restore_file_and_refresh_entry(parent_key: &str,
                                  name: &str,
                                  hash: &str,
                                  item_type: DirEntryType) -> Result<(), String> {
    let file_path = if parent_key.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", parent_key, name)
    };

    shift_utils::write_tracked_file(&file_path, hash, item_type)?;

    let refreshed = inventory_utils::build_inventory_item_from_stat(
        std::path::Path::new(&file_path),
        name,
        hash.to_string()
    )?;

    inventory_utils::update_shard(parent_key, |inventory| {
        inventory.add_item(refreshed);
        Ok(())
    })
}

/// Reset the inventory entries of the given path to the pallet head (unstage). The
/// working directory is not touched; the reset entries carry zeroed stat data, so the
/// next comparison against the working directory rehashes them.
///
/// # Arguments
/// * `path` - The path to unstage.
///
/// # Returns
/// * `Ok(())`      - If the unstage completed.
/// * `Err(String)` - If the path exists neither in the inventory nor in the head.
fn restore_staged(path: &WarehousePath) -> Result<(), String> {
    let pallet = pallet_utils::get_current_pallet_name()?;
    let head = pallet_utils::get_pallet_head(&pallet)?;

    let head_tree_hash = match &head {
        Some(hash) => Some(object_utils::load_parcel(hash)?.tree_hash),
        None => None,
    };

    let head_entry = match &head_tree_hash {
        Some(tree_hash) => resolve_head_entry(tree_hash, path.as_key())?,
        None => None,
    };

    let has_shard = file_utils::get_inventory_data_path_for_key(path.as_key()).exists();
    let treat_as_directory = path.is_root()
        || has_shard
        || matches!(head_entry, Some(HeadEntry::Tree(_)));

    if treat_as_directory {
        // Rebuild the whole subtree of the staging area from the head: directories that
        // exist only in the inventory disappear, directories that exist only in the head
        // come back (with stale stat data).
        let mut shards: BTreeMap<String, Inventory> = BTreeMap::new();

        if let Some(HeadEntry::Tree(tree)) = &head_entry {
            build_stale_shards(tree, path.as_key(), &mut shards)?;
        }

        inventory_utils::replace_subtree_inventories(path.as_key(), &shards)?;

        output::message("restore", format!(
            "Unstaged \"{}\" (inventory reset to the pallet head).",
            if path.is_root() { "./" } else { path.as_key() }
        ));

        return Ok(());
    }

    // A single file: reset its entry from the head, or drop it if the head lacks it.
    let (parent, file_name) = path.split_parent()?;

    match head_entry {
        Some(HeadEntry::File { hash, item_type }) => {
            inventory_utils::update_shard(parent.as_key(), |inventory| {
                inventory.add_item(inventory_utils::build_stale_inventory_item(
                    &file_name, hash, item_type
                ));
                Ok(())
            })?;
        }
        Some(HeadEntry::Tree(_)) => unreachable!("directories are handled above"),
        None => {
            let mut removed = false;

            inventory_utils::update_shard(parent.as_key(), |inventory| {
                removed = inventory.remove_item_by_name(&file_name);
                Ok(())
            })?;

            if !removed {
                return Err(format!(
                    "\"{}\" is neither in the inventory nor in the pallet head.",
                    path.as_key()
                ));
            }
        }
    }

    output::message("restore", format!("Unstaged \"{}\".", path.as_key()));

    Ok(())
}

/// Build stale-stat inventory shards for a head subtree (see `build_stale_inventory_item`).
///
/// # Arguments
/// * `tree`   - The (loaded) head tree of the directory.
/// * `key`    - The warehouse path key of the directory.
/// * `shards` - The collected shards.
///
/// # Returns
/// * `Ok(())`      - If the shards were built.
/// * `Err(String)` - If a subtree object could not be loaded.
fn build_stale_shards(tree: &TreeItem,
                      key: &str,
                      shards: &mut BTreeMap<String, Inventory>) -> Result<(), String> {
    let mut inventory = Inventory::new();

    for (name, item) in tree.get_files() {
        inventory.add_item(inventory_utils::build_stale_inventory_item(
            name,
            item.hash.clone(),
            item.item_type
        ));
    }

    shards.insert(key.to_string(), inventory);

    for (name, subtree) in tree.get_subtrees() {
        let child_key = if key.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", key, name)
        };

        let subtree_loaded = object_utils::load_tree(&subtree.hash)?;
        build_stale_shards(&subtree_loaded, &child_key, shards)?;
    }

    Ok(())
}

/// Resolve a warehouse path inside the head tree.
///
/// # Arguments
/// * `root_tree_hash` - The hash of the head parcel's root tree.
/// * `key`            - The warehouse path key to resolve (`""` resolves to the root tree).
///
/// # Returns
/// * `Ok(Some(HeadEntry))` - The resolved entry (a file or a loaded directory).
/// * `Ok(None)`            - If the path does not exist in the head tree.
/// * `Err(String)`         - If a tree object could not be loaded.
fn resolve_head_entry(root_tree_hash: &str, key: &str) -> Result<Option<HeadEntry>, String> {
    let mut current = object_utils::load_tree(root_tree_hash)?;

    if key.is_empty() {
        return Ok(Some(HeadEntry::Tree(current)));
    }

    let components: Vec<&str> = key.split(file_utils::PATH_SEPARATOR_CHAR).collect();

    for (index, component) in components.iter().enumerate() {
        let is_last = index == components.len() - 1;

        if is_last {
            if let Some((_, item)) = current.get_files().find(|(name, _)| name == component) {
                return Ok(Some(HeadEntry::File {
                    hash: item.hash.clone(),
                    item_type: item.item_type,
                }));
            }
        }

        let subtree = current.get_subtrees()
            .find(|(name, _)| name == component)
            .map(|(_, item)| item.hash.clone());

        match subtree {
            Some(subtree_hash) => current = object_utils::load_tree(&subtree_hash)?,
            None => return Ok(None),
        }
    }

    Ok(Some(HeadEntry::Tree(current)))
}
