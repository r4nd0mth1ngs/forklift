use crate::enums::object::tree_version::TreeVersion;
use crate::model::tree_item::TreeItem;
use crate::util::byte_utils;

/// Parse a tree object.
///
/// # Arguments
/// * `offset`  - The offset to start parsing at. This should be the byte *after*
/// the header section.
/// * `content` - The bytes of the tree object.
///
/// # Returns
/// * `Ok(TreeItem)`  - The parsed tree object.
/// * `Err(String)`   - The error message.
pub fn parse_tree(offset: usize, content: &[u8]) -> Result<TreeItem, String> {
    let mut cursor = 0;

    let version_code = byte_utils::number_from_vlq_bytes(offset, content)
        .map(|(value, bytes_read)| {
            cursor += bytes_read;
            value
        }).map_err(|e| format!("Failed to parse version code: {}", e))?;

    let version = TreeVersion::from_code(version_code)?;
    let tree = version.get_parser()(offset + cursor, content);

    tree
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::object::tree::tree_object_builder::TreeObjectBuilder;
    use crate::enums::dir_entry_type::DirEntryType;

    #[test]
    fn round_trip_preserves_entries_and_hostile_names() {
        let hash = "9028a15ad613bcd9853a3e780cfe3c78361b56ce95a2430484ba75ade5198cdc";

        let mut tree = TreeItem::new(String::new(), String::new(), DirEntryType::Tree);
        tree.add_child(TreeItem::new("normal.txt".to_string(), hash.to_string(), DirEntryType::Normal));
        // Names containing new line and EOT bytes must survive the round trip.
        tree.add_child(TreeItem::new("with\nnewline".to_string(), hash.to_string(), DirEntryType::Executable));
        tree.add_child(TreeItem::new("with\u{3}end-of-text".to_string(), hash.to_string(), DirEntryType::Normal));
        tree.add_child(TreeItem::new("subdir".to_string(), hash.to_string(), DirEntryType::Tree));

        let object = TreeObjectBuilder::build(&tree);
        let parsed = parse_tree(0, &object.content).unwrap();

        assert_eq!(parsed.get_files().count(), 3);
        assert_eq!(parsed.get_subtrees().count(), 1);

        for (name, item) in parsed.get_files().chain(parsed.get_subtrees()) {
            assert_eq!(item.hash, hash, "hash mismatch for entry {:?}", name);
        }

        let file_names: Vec<&String> = parsed.get_files().map(|(name, _)| name).collect();
        assert!(file_names.iter().any(|n| *n == "with\nnewline"));
        assert!(file_names.iter().any(|n| *n == "with\u{3}end-of-text"));
    }
}
