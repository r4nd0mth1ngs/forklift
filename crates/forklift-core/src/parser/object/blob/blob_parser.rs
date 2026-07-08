use crate::enums::object::blob_version::BlobVersion;
use crate::model::blob::Blob;
use crate::util::byte_utils;

/// Parse a blob object.
///
/// # Arguments
/// * `offset`  - The offset to start parsing at. This should be the byte *after*
/// the header section.
/// * `content` - The bytes of the blob object.
///
/// # Returns
/// * `Ok(Blob)`    - The parsed blob object.
/// * `Err(String)` - The error message.
pub fn parse_blob(offset: usize, content: &[u8]) -> Result<Blob, String> {
    let mut cursor: usize = 0;

    let version_code = byte_utils::number_from_vlq_bytes(offset + cursor, content)
        .map(|(value, bytes_read)| {
            cursor += bytes_read;
            value
        })
        .map_err(|e| format!("Failed to parse blob version: {}", e))?;

    let version = BlobVersion::from_code(version_code)?;
    let blob = version.get_parser()(offset + cursor, content);

    blob
}