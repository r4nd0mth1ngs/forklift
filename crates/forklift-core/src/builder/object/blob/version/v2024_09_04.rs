use crate::model::blob::Blob;
use crate::util::byte_utils;

/// Build a blob object with version `V2024_09_04`.
///
/// # Arguments
/// * `version` - The version of the object.
/// * `blob`    - The blob data.
///
/// # Returns
/// The bytes of the blob object.
pub fn build(version: u64, blob: &Blob) -> Vec<u8> {
    let mut content: Vec<u8> = Vec::new();

    content.extend(byte_utils::number_to_vlq_bytes(version));

    content.extend(blob.content.as_slice());

    content
}