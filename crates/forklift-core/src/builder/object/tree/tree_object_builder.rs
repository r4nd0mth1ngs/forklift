use crate::enums::object::tree_version::TreeVersion;
use crate::model::object::tree_object::TreeObject;
use crate::model::tree_item::TreeItem;

/// Builder for tree objects.
/// This should NOT be used directly. Use `LooseObjectBuilder` instead.
pub struct TreeObjectBuilder {
    pub content: Vec<u8>
}

impl TreeObjectBuilder {
    /// Build a tree object.
    ///
    /// # Arguments
    /// * `tree` - The tree data.
    ///
    /// # Returns
    /// The built tree object.
    pub fn build(tree: &TreeItem) -> TreeObject {
        let version = TreeVersion::latest();
        let builder_fn = version.get_builder();

        TreeObject {
            content: builder_fn(tree)
        }
    }
}