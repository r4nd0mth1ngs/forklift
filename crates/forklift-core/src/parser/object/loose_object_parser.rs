use crate::enums::object::loose_object_version::LooseObjectVersion;
use crate::enums::object::parsed_object::ParsedObject;
use crate::model::blob::Blob;
use crate::model::parcel::Parcel;
use crate::model::tree_item::TreeItem;
use crate::util::byte_utils;

/// Parse a parcel object from the given loose object bytes.
///
/// # Arguments
/// * `input` - The bytes of the loose object.
///
/// # Returns
/// * `Ok(Parcel)`  - The parsed parcel object.
/// * `Err(String)` - The error message.
pub fn parse_parcel(input: &[u8]) -> Result<Parcel, String> {
    let mut cursor = 0;

    let object_format_version = read_version(input)
        .map(|(version, bytes_read)| {
            cursor += bytes_read;
            version
        })?;

    let parcel = object_format_version.get_parcel_parser()(cursor, input);

    parcel
}

/// Parse a blob object from the given loose object bytes.
///
/// # Arguments
/// * `input` - The bytes of the loose object.
///
/// # Returns
/// * `Ok(Blob)`    - The parsed blob object.
/// * `Err(String)` - The error message.
pub fn parse_blob(input: &[u8]) -> Result<Blob, String> {
    let mut cursor = 0;

    let object_format_version = read_version(input)
        .map(|(version, bytes_read)| {
            cursor += bytes_read;
            version
        })?;

    let blob = object_format_version.get_blob_parser()(cursor, input);

    blob
}

/// Parse a tree object from the given loose object bytes.
///
/// # Arguments
/// * `input` - The bytes of the loose object.
///
/// # Returns
/// * `Ok(Tree)`    - The parsed tree object.
/// * `Err(String)` - The error message.
pub fn parse_tree(input: &[u8]) -> Result<TreeItem, String> {
    let mut cursor = 0;

    let object_format_version = read_version(input)
        .map(|(version, bytes_read)| {
            cursor += bytes_read;
            version
        })?;

    let tree = object_format_version.get_tree_parser()(cursor, input);

    tree
}

/// Parse any loose object from the given bytes.
///
/// # Arguments
/// * `input` - The bytes of the loose object.
///
/// # Returns
/// * `Ok(ParsedObject)` - The parsed object.
/// * `Err(String)`      - The error message.
pub fn parse(input: &[u8]) -> Result<ParsedObject, String> {
    let mut cursor = 0;

    let object_format_version = read_version(input)
        .map(|(version, bytes_read)| {
            cursor += bytes_read;
            version
        })?;

    let object = object_format_version.get_parser()(cursor, input);

    object
}

/// Read the object format version from the given loose object bytes.
///
/// # Arguments
/// * `input` - The bytes of the loose object.
///
/// # Returns
/// * `Ok((LooseObjectVersion, usize))`:
///    * `LooseObjectVersion` - The object format version.
///    * `usize`              - The number of bytes read.
/// * `Err(String)` - The error message.
fn read_version(input: &[u8]) -> Result<(LooseObjectVersion, usize), String> {
    let mut cursor = 0;

    let object_format_version_code = byte_utils::number_from_vlq_bytes(cursor, input)
        .map(|(value, bytes_read)| {
            cursor += bytes_read;
            value
        })
        .map_err(|_| "Failed to parse object format version.".to_string())?;

    LooseObjectVersion::from_code(object_format_version_code)
        .map(|version| (version, cursor))
}