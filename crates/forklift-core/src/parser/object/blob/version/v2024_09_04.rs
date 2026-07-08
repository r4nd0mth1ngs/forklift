use crate::model::blob::Blob;

/// Parse a blob object with version `V2024_09_04`.
///
/// # Arguments
/// * `offset` - The offset to start parsing at This should be the byte *after*
/// the blob format version code.
/// * `input`  - The bytes of the blob object.
///
/// # Returns
/// * `Ok(Blob)`    - The parsed blob object.
/// * `Err(String)` - The error message.
pub fn parse(offset: usize, input: &[u8]) -> Result<Blob, String> {
    Ok (
        Blob {
            content: input[offset..].to_vec()
        }
    )
}