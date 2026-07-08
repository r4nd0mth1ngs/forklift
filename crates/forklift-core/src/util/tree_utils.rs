use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::ops::Add;
use std::path::PathBuf;
use std::sync::Arc;
use regex::Regex;
use crate::builder::object::loose_object_builder::LooseObjectBuilder;
use crate::enums::dir_entry_type::DirEntryType;
use crate::enums::inventory_item_state::InventoryItemState;
use crate::model::task::tree_builder::tree_builder_context::TreeBuilderContext;
use crate::model::task::TaskExecutor;
use crate::model::tree_item::TreeItem;
use crate::parser;
use crate::traits::task_context::TaskContext;
use crate::types::task::Task;
use crate::util::{file_utils, inventory_utils, object_utils};

const FILENAME_METADATA_SUFFIX: &str = ".metadata";

/// Build (and store) tree objects from the inventory, bottom-up: one tree object per
/// inventoried directory. This is the first half of stacking a parcel.
///
/// * Entries staged for removal (`Deleted`) are excluded — that is how a staged removal
///   becomes an actual removal in the next parcel.
/// * Directories that end up empty (no files, no non-empty subdirectories) are pruned,
///   except the warehouse root.
/// * Ancestor directories that have no shard of their own (e.g. only `src/a` was ever
///   loaded) are synthesized so the chain root → `src` → `a` exists in the tree.
///
/// # Returns
/// * `Ok(Some(TreeItem))` - The root tree (its hash set, all tree objects stored).
/// * `Ok(None)`           - If there is no inventory at all (nothing was ever loaded).
/// * `Err(String)`        - If a shard could not be read or an object could not be stored.
///
/// The build runs in parallel over the `TaskExecutor` (one task per directory), scheduled
/// bottom-up by dependency: the leaves are enqueued first, and each completing directory
/// enqueues its parent once the parent's last child is built.
pub async fn build_tree_from_inventory() -> Result<Option<TreeItem>, String> {
    let (_, metadata_opt) = file_utils::retrieve_inventory_metadata_or_none()?;

    let Some(metadata) = metadata_opt else {
        return Ok(None);
    };

    if metadata.is_empty() {
        return Ok(None);
    }

    // Collect every inventoried directory key plus all of its ancestors (ancestors may
    // have no shard of their own), then derive the parent → children relation.
    let mut keys: BTreeSet<String> = BTreeSet::new();

    for entry in &metadata {
        let mut key = inventory_utils::metadata_entry_to_key(entry);

        loop {
            keys.insert(key.to_string());

            match key.rsplit_once(file_utils::PATH_SEPARATOR_CHAR) {
                Some((parent, _)) => key = parent,
                None => break,
            }
        }

        keys.insert(String::new());
    }

    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for key in &keys {
        if key.is_empty() {
            continue;
        }

        let parent = key.rsplit_once(file_utils::PATH_SEPARATOR_CHAR)
            .map(|(parent, _)| parent)
            .unwrap_or("");

        children.entry(parent.to_string()).or_default().push(key.clone());
    }

    let children = Arc::new(children);

    // Dependency counters: a directory becomes buildable once all of its children are
    // built. Directories without children (the leaves) are buildable immediately.
    let pending_children: HashMap<String, usize> = keys.iter()
        .map(|key| (key.clone(), children.get(key).map(|c| c.len()).unwrap_or(0)))
        .collect();

    let mut leaves: Vec<String> = keys.iter()
        .filter(|key| children.get(*key).map(|c| c.is_empty()).unwrap_or(true))
        .cloned()
        .collect();

    let context = Arc::new(TreeBuilderContext::new(pending_children));
    let executor = TaskExecutor::new(Arc::clone(&context));

    let first_leaf = leaves.pop()
        .ok_or("The tree build found no leaf directory to start from.".to_string())?;

    for leaf in leaves {
        context.send_task(Box::pin(build_tree_for_inventory_key(
            Arc::clone(&context),
            leaf,
            Arc::clone(&children),
        )))?;
    }

    let root_task: Task<(), String> = Box::pin(build_tree_for_inventory_key(
        Arc::clone(&context),
        first_leaf,
        Arc::clone(&children),
    ));

    executor.execute(root_task).await.map_err(|e|
        e.unwrap_or("An unknown error occurred while building the trees.".to_string())
    )?;

    let root = context.built.lock().await.remove("")
        .ok_or("The tree build finished without producing a root tree.".to_string())?;

    Ok(Some(root))
}

/// The per-directory task of the tree build: build (and store) the tree object for one
/// inventoried directory, taking the already-built child trees from the shared context,
/// and enqueue the parent's task when this was the parent's last unbuilt child.
///
/// Empty subtrees are pruned (`add_child` is skipped by the parent), but every built
/// tree object is stored, and the root is always kept, even when empty.
///
/// # Arguments
/// * `context`  - The shared build context.
/// * `key`      - The warehouse path key of the directory.
/// * `children` - The parent key → child keys relation over all inventoried directories.
///
/// # Returns
/// * `Ok(())`      - If the directory's tree was built and stored.
/// * `Err(String)` - If a shard could not be read or an object could not be stored.
fn build_tree_for_inventory_key(context: Arc<TreeBuilderContext>,
                                key: String,
                                children: Arc<BTreeMap<String, Vec<String>>>)
                                -> impl Future<Output = Result<(), String>> + Send {
    async move {
        let name = key.rsplit_once(file_utils::PATH_SEPARATOR_CHAR)
            .map(|(_, name)| name)
            .unwrap_or(&key);

        let mut tree = TreeItem::new(name.to_string(), String::new(), DirEntryType::Tree);

        let (_, shard_bytes) = file_utils::retrieve_inventory_or_none_by_key(&key)?;

        if let Some(bytes) = shard_bytes {
            let inventory = parser::inventory::inventory_parser::parse_inventory(&bytes)
                .map_err(|e| format!("Error while parsing the inventory of folder \"{}\": {}", key, e))?;

            for (_, item) in inventory.get_items() {
                if item.state == InventoryItemState::Deleted {
                    continue;
                }

                tree.add_child(TreeItem::new(item.name.clone(), item.hash.clone(), item.item_type));
            }
        }

        if let Some(child_keys) = children.get(&key) {
            let mut built = context.built.lock().await;

            for child_key in child_keys {
                let child_tree = built.remove(child_key)
                    .ok_or(format!("Subtree \"{}\" was not built before its parent.", child_key))?;

                let is_empty = child_tree.get_files().len() == 0
                    && child_tree.get_subtrees().len() == 0;

                if !is_empty {
                    tree.add_child(child_tree);
                }
            }
        }

        let mut object = LooseObjectBuilder::build_tree(&tree);
        tree.hash = object.hash.clone();
        object.store()?;

        context.built.lock().await.insert(key.clone(), tree);

        // The parent becomes buildable once its last child is built.
        if !key.is_empty() {
            let parent = key.rsplit_once(file_utils::PATH_SEPARATOR_CHAR)
                .map(|(parent, _)| parent)
                .unwrap_or("")
                .to_string();

            let is_parent_ready = {
                let mut pending = context.pending_children.lock().await;
                let counter = pending.get_mut(&parent)
                    .ok_or(format!("No pending-children counter for directory \"{}\".", parent))?;

                *counter -= 1;
                *counter == 0
            };

            if is_parent_ready {
                context.send_task(Box::pin(build_tree_for_inventory_key(
                    Arc::clone(&context),
                    parent,
                    Arc::clone(&children),
                )))?;
            }
        }

        Ok(())
    }
}

/// Create tree objects for the given directory and all of its subdirectories.
/// The tree objects are stored in the object store.
/// A metadata file is also created, which contains the mapping of directory paths to tree hashes.
///
/// # Arguments
/// * `path` - The path to the directory.
///
/// # Returns
/// * `Ok(TreeItem)` - if the tree was built successfully.
/// * `Err(String)`  - if an error occurred while building the tree.
pub fn create_tree_for_directory(path: &PathBuf) -> Result<Option<TreeItem>, String> {
    let mut tree_hashes: BTreeMap<String, String> = BTreeMap::new();
    let ignored_paths = file_utils::get_ignored_paths()?;
    let result = build_tree(path, &mut tree_hashes, &ignored_paths)?;

    if let Some(tree_item) = &result {
        build_tree_metadata(&tree_item.hash, &tree_hashes)?;
    }

    Ok(result)
}

/// Build a tree item from a directory.
/// Created tree and blob objects are stored.
///
/// # Arguments
/// * `path`        - The path to the directory.
/// * `tree_hashes` - The mapping of directory paths to tree hashes.
/// The new tree objects will be added to this map.
///
/// # Returns
/// * `Ok(TreeItem)` - if the tree was built successfully.
/// * `Err(String)`  - if an error occurred while building the tree.
fn build_tree(path: &PathBuf,
              tree_hashes: &mut BTreeMap<String, String>,
              ignored_paths: &Vec<Regex>) -> Result<Option<TreeItem>, String> {
    let path_string = file_utils::path_to_string(path)?;

    if ignored_paths.iter().any(|r| r.is_match(&path_string)) {
        return Ok(None)
    }

    let directory = file_utils::read_directory(path)?;
    let name = file_utils::get_filename_from_path(path)?.unwrap_or(String::new());

    let mut tree = TreeItem::new(name, String::from(""), DirEntryType::Tree);

    for entry_result in directory {
        let entry = entry_result
            .map_err(|e| format!("Error while reading directory entry: {}", e))?;
        let entry_path = file_utils::path_to_string(&entry.path())?;

        if ignored_paths.iter().any(|r| r.is_match(&entry_path)) {
            continue;
        }

        let name = file_utils::get_name_for_file_or_directory(&entry)?;
        let metadata = file_utils::get_symlink_metadata_for_path(&entry.path())?;
        let item_type = file_utils::get_type_of_dir_entry(&metadata);

        if item_type.is_file() {
            let tree_item = build_tree_item_from_file(&entry, name, item_type)?;
            tree.add_child(tree_item);
        } else {
            let tree_item = build_tree(&entry.path(), tree_hashes, ignored_paths)?;

            if let Some(item) = tree_item {
                tree.add_child(item);
            }
        }
    }

    let mut object = LooseObjectBuilder::build_tree(&tree);
    tree.hash = object.hash.clone();
    object.store()?;

    let path_string = file_utils::path_to_string(path)?;
    tree_hashes.insert(path_string, object.hash.clone());

    Ok(Some(tree))
}

/// Build a tree item from a file.
/// Created blob objects are stored.
///
/// # Arguments
/// * `entry`     - The directory entry to build the tree item from (should be a file).
/// * `name`      - The name of the file.
/// * `item_type` - The type of the tree item.
///
/// # Returns
/// * `Ok(TreeItem)` - if the tree item was built successfully.
/// * `Err(String)`  - if an error occurred while building the tree item.
fn build_tree_item_from_file(entry: &std::fs::DirEntry,
                             name: String,
                             item_type: DirEntryType) -> Result<TreeItem, String> {
    let blob = object_utils::get_blob_for_file(&name, &entry.path(), &item_type)?;

    let mut object = LooseObjectBuilder::build_blob(&blob);
    object.store()?;

    Ok(TreeItem::new(name, object.hash, item_type))
}

/// Create (and save) a tree metadata file.
/// The metadata file contains the mapping of directory paths to tree hashes.
///
/// # Arguments
/// * `root_hash`   - The hash of the root tree object.
/// * `tree_hashes` - The mapping of directory paths to tree hashes.
/// The key should be the path, and the value should be the hash.
///
/// # Returns
/// * `Ok(())`      - if the metadata was successfully created.
/// * `Err(String)` - if an error occurred while creating the metadata.
fn build_tree_metadata(root_hash: &str, tree_hashes: &BTreeMap<String, String>) -> Result<(), String> {
    let mut metadata: Vec<u8>  = Vec::new();

    for (path, hash) in tree_hashes {
        metadata.extend(path.as_bytes());
        object_utils::push_end_of_text(&mut metadata);
        metadata.extend_from_slice(hash.as_bytes());
        object_utils::push_new_line(&mut metadata);
    }

    let (folder_path, tree_filename) = file_utils::get_path_for_object(root_hash)?;
    let metadata_path = String::from(folder_path)
        .add(file_utils::PATH_SEPARATOR)
        .add(&tree_filename)
        .add(FILENAME_METADATA_SUFFIX);

    std::fs::write(&metadata_path, metadata).map_err(|e|
        format!("Error while writing tree metadata to file \"{}\": {}", metadata_path, e)
    )?;

    Ok(())
}
