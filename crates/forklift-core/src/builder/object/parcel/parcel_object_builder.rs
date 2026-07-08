use crate::enums::object::compact_parcel_version::CompactParcelVersion;
use crate::model::object::parcel_object::ParcelObject;
use crate::model::parcel::Parcel;

/// Builder for parcel objects.
/// This should NOT be used directly. Use `LooseObjectBuilder` instead.
pub struct ParcelObjectBuilder {
    pub content: Vec<u8>,
}

impl ParcelObjectBuilder {
    /// Build a compact parcel object.
    ///
    /// # Arguments
    /// * `parcel` - The parcel data.
    ///
    /// # Returns
    /// The built parcel object.
    pub fn build_compact(parcel: &Parcel) -> ParcelObject {
        let version = CompactParcelVersion::latest();
        let builder_fn = version.get_builder();

        ParcelObject {
            content: builder_fn(parcel),
        }
    }
}