use crate::model::parcel::Parcel;
use crate::util::{byte_utils, object_utils};

/// Build a parcel object with version `V2024_09_04`.
///
/// # Arguments
/// * `version` - The version of the object.
/// * `parcel`  - The parcel data.
///
/// # Returns
/// The bytes of the parcel object.
pub fn build(version: u64, parcel: &Parcel) -> Vec<u8> {
    let mut content: Vec<u8> = Vec::new();

    content.extend(byte_utils::number_to_vlq_bytes(version));
    content.extend(parcel.tree_hash.as_bytes().to_vec());
    object_utils::push_new_line(&mut content);

    parcel.parents.iter().for_each(|parent| {
        content.extend(parent.as_bytes());
        object_utils::push_new_line(&mut content);
    });

    // End of parents section.
    object_utils::push_null(&mut content);

    parcel.actions.iter().for_each(|action| {
        content.extend(byte_utils::number_to_vlq_bytes(action.action.get_code()));
        content.extend(byte_utils::number_to_vlq_bytes(action.timestamp.timestamp() as u64));
        content.extend(action.operator.identifier.as_bytes());
        object_utils::push_end_of_text(&mut content);

        if let Some(description) = &action.description {
            content.extend(description.as_bytes());
        }

        object_utils::push_new_line(&mut content);
    });

    // End of actions section.
    object_utils::push_null(&mut content);

    if let Some(description) = &parcel.description {
        content.extend(description.as_bytes());
    }

    content
}