use crate::enums::object::parsed_object::ParsedObject;
use crate::model::blob::Blob;
use crate::model::parcel::Parcel;
use crate::model::tree_item::TreeItem;
use crate::parser;

const CODE_VERSION_2024_09_04: u64 = 1;

/// Versions of the loose object format.
/// This is the format of the overall object (e.g. the structure of the header).
/// The contents of the object must have a separate version number.
pub enum LooseObjectVersion {
    /// The original version, defined on 09/04/2024.
    V2024_09_04,
}

impl LooseObjectVersion {
    /// Get the code of the version.
    ///
    /// # Returns
    /// * `u64` - The code of the version.
    pub fn get_code(&self) -> u64 {
        match self {
            LooseObjectVersion::V2024_09_04 => CODE_VERSION_2024_09_04,
        }
    }

    /// Get the version for the given code.
    ///
    /// # Arguments
    /// * `code` - The code of the version.
    ///
    /// # Returns
    /// * `Ok(LooseObjectVersion)` - The version associated with the given code.
    /// * `Err(String)`            - The error message if the version is unknown.
    pub fn from_code(code: u64) -> Result<LooseObjectVersion, String> {
        match code {
            CODE_VERSION_2024_09_04 => Ok(LooseObjectVersion::V2024_09_04),
            _ => Err(format!("Unknown loose object version: {}", code)),
        }
    }

    /// Get the parcel object parser function for the version.
    ///
    /// # Returns
    /// * `impl Fn(usize, &[u8]) -> Result<Parcel, String>` - The object parser function.
    pub fn get_parcel_parser(&self) -> impl Fn(usize, &[u8]) -> Result<Parcel, String> + '_ {
        let parser_fn = match self {
            LooseObjectVersion::V2024_09_04 =>
                parser::object::version::v2024_09_04::parse_parcel
        };

        move |offset: usize, content: &[u8]| parser_fn(offset, content)
    }

    /// Get the blob object parser function for the version.
    ///
    /// # Returns
    /// * `impl Fn(usize, &[u8]) -> Result<Blob, String>` - The object parser function.
    pub fn get_blob_parser(&self) -> impl Fn(usize, &[u8]) -> Result<Blob, String> + '_ {
        let parser_fn = match self {
            LooseObjectVersion::V2024_09_04 =>
                parser::object::version::v2024_09_04::parse_blob
        };

        move |offset: usize, content: &[u8]| parser_fn(offset, content)
    }

    /// Get the tree object parser function for the version.
    ///
    /// # Returns
    /// * `impl Fn(usize, &[u8]) -> Result<TreeItem, String>` - The object parser function.
    pub fn get_tree_parser(&self) -> impl Fn(usize, &[u8]) -> Result<TreeItem, String> + '_ {
        let parser_fn = match self {
            LooseObjectVersion::V2024_09_04 =>
                parser::object::version::v2024_09_04::parse_tree
        };

        move |offset: usize, content: &[u8]| parser_fn(offset, content)
    }

    /// Get the generic object parser function for the version.
    ///
    /// # Returns
    /// * `impl Fn(usize, &[u8]) -> Result<ParsedObject, String>` - The object parser function.
    pub fn get_parser(&self) -> impl Fn(usize, &[u8]) -> Result<ParsedObject, String> + '_ {
        let parser_fn = match self {
            LooseObjectVersion::V2024_09_04 =>
                parser::object::version::v2024_09_04::parse
        };

        move |offset: usize, content: &[u8]| parser_fn(offset, content)
    }

    /// Get the latest version of the loose object format.
    ///
    /// # Returns
    /// The latest version of the loose object format.
    pub fn latest() -> LooseObjectVersion {
        LooseObjectVersion::V2024_09_04
    }
}