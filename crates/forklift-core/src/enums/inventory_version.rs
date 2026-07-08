use crate::{builder, parser};
use crate::model::inventory::Inventory;

const CODE_VERSION_2024_09_04: u64 = 1;

/// Versions of the inventory object format.
pub enum InventoryVersion {
    /// The original version, defined on 09/04/2024.
    V2024_09_04,
}

impl InventoryVersion {
    /// Get the code of the inventory version.
    ///
    /// # Returns
    /// * `u64` - The code of the inventory version.
    pub fn get_code(&self) -> u64 {
        match self {
            InventoryVersion::V2024_09_04 => CODE_VERSION_2024_09_04,
        }
    }

    /// Get the inventory version for the given code.
    ///
    /// # Arguments
    /// * `code` - The code of the inventory version.
    ///
    /// # Returns
    /// * `Ok(InventoryVersion)` - The inventory version.
    /// * `Err(String)`          - If the code is not recognized.
    pub fn from_code(code: u64) -> Result<InventoryVersion, String> {
        match code {
            CODE_VERSION_2024_09_04 => Ok(InventoryVersion::V2024_09_04),
            _ => Err(format!("Inventory version code {} not found.", code)),
        }
    }

    /// Get the function for building inventory files with the given version.
    ///
    /// # Returns
    /// * `impl Fn(&Inventory) -> Vec<u8>` - The builder function.
    pub fn get_builder(&self) -> impl Fn(&Inventory) -> Vec<u8> + '_ {
        let builder_fn = match self {
            InventoryVersion::V2024_09_04 => builder::inventory::version::v2024_09_04::build
        };

        move |inventory| builder_fn(inventory)
    }

    /// Get the function for parsing inventory files with the given version.
    ///
    /// # Returns
    /// * `impl Fn(usize, &[u8]) -> Result<Inventory, String>` - The parser function.
    pub fn get_parser(&self) -> impl Fn(usize, &[u8]) -> Result<Inventory, String> + '_ {
        let parser_fn = match self {
            InventoryVersion::V2024_09_04 => parser::inventory::version::v2024_09_04::parse
        };

        move |offset, content| parser_fn(offset, content)
    }

    /// Get the latest inventory version.
    ///
    /// # Returns
    /// * `InventoryVersion` - The latest inventory version.
    pub fn latest() -> InventoryVersion {
        InventoryVersion::V2024_09_04
    }
}