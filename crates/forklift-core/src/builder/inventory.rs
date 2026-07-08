use crate::enums::inventory_version::InventoryVersion;
use crate::model::inventory::Inventory;
use crate::util::{byte_utils, object_utils};

pub mod version;

/// A builder for inventory objects.
pub struct InventoryBuilder {
    pub version: InventoryVersion,
    pub content: Vec<u8>,
}

impl InventoryBuilder {
    /// Build an inventory object.
    ///
    /// # Arguments
    /// * `inventory` - The inventory to build.
    ///
    /// # Returns
    /// * `Vec<u8>` - The bytes of the inventory object (including the header).
    pub fn build(inventory: &Inventory) -> Vec<u8> {
        let builder = InventoryBuilder::new(InventoryVersion::latest());

        builder.write_header(inventory.get_items_count() as u64).write_content(inventory).content
    }

    /// Create a new inventory builder.
    ///
    /// # Arguments
    /// * `version` - The inventory file version to use.
    ///
    /// # Returns
    /// * `InventoryBuilder` - The inventory builder.
    fn new(version: InventoryVersion) -> InventoryBuilder {
        InventoryBuilder {
            content: Vec::new(),
            version,
        }
    }

    /// Write the header to the bytes of the inventory object.
    ///
    /// # Arguments
    /// * `entry_count` - The number of entries in the inventory.
    ///
    /// # Returns
    /// * `InventoryBuilder` - The inventory builder.
    fn write_header(mut self, entry_count: u64) -> Self {
        self.content.extend(byte_utils::number_to_vlq_bytes(self.version.get_code()));
        self.content.extend(byte_utils::number_to_vlq_bytes(entry_count));
        object_utils::push_null(&mut self.content);

        self
    }

    /// Write the content to the bytes of the inventory object.
    ///
    /// # Arguments
    /// * `inventory` - The inventory to write to the object.
    ///
    /// # Returns
    /// * `InventoryBuilder` - The inventory builder.
    fn write_content(mut self, inventory: &Inventory) -> Self {
        self.content.extend(self.version.get_builder()(inventory));

        self
    }
}