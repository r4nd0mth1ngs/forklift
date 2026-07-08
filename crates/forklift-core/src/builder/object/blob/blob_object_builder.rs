use crate::enums::object::blob_version::BlobVersion;
use crate::model::blob::Blob;
use crate::model::object::blob_object::BlobObject;

/// Builder for blob objects.
/// This should NOT be used directly. Use `LooseObjectBuilder` instead.
pub struct BlobObjectBuilder {
    pub content: Vec<u8>,
}

impl BlobObjectBuilder {
    /// Build a blob object.
    ///
    /// # Arguments
    /// * `blob` - The blob data.
    ///
    /// # Returns
    /// The built blob object.
    pub fn build(blob: &Blob) -> BlobObject {
        let version = BlobVersion::latest();
        let builder_fn = version.get_builder();

        BlobObject {
            content: builder_fn(blob),
        }
    }
}