use std::collections::btree_map::Iter;
use crate::model::tree_item::TreeItem;
use crate::util::{byte_utils, object_utils};

/// Build a tree object with version `V2024_09_04`.
///
/// # Arguments
/// * `version` - The version of the object.
/// * `tree`    - The tree item data.
///
/// # Returns
/// The bytes of the tree object.
pub fn build(version: u64, tree: &TreeItem) -> Vec<u8> {
    let mut content: Vec<u8> = Vec::new();

    content.extend(byte_utils::number_to_vlq_bytes(version));

    add_items(&mut content, tree.get_subtrees());
    add_items(&mut content, tree.get_files());

    content
}

/// Add the given tree items to the content.
///
/// # Arguments
/// * `content` - The content to add the items to.
/// * `items`   - The items to add.
fn add_items(content: &mut Vec<u8>, items: Iter<String, TreeItem>) {
    for (name, subtree) in items {
        content.extend(byte_utils::number_to_vlq_bytes(subtree.item_type.get_code()));
        content.extend(name.as_bytes());
        object_utils::push_end_of_text(content);
        content.extend(subtree.hash.as_bytes());
        object_utils::push_new_line(content);
    }
}