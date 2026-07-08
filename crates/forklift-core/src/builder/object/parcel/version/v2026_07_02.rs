use crate::model::parcel::Parcel;
use crate::util::{byte_utils, object_utils};

/// Build a parcel object with version `V2026_07_02`.
///
/// This version length-prefixes the operator identifier and the action description instead
/// of using EOT / new line terminators: both values are user-controlled and may contain any
/// byte, so terminator bytes cannot delimit them safely. A description length of `0` means
/// the action has no description.
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
    content.extend(parcel.tree_hash.as_bytes());
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

        let operator = action.operator.identifier.as_bytes();
        content.extend(byte_utils::number_to_vlq_bytes(operator.len() as u64));
        content.extend(operator);

        let description = action.description.as_deref().unwrap_or("").as_bytes();
        content.extend(byte_utils::number_to_vlq_bytes(description.len() as u64));
        content.extend(description);
    });

    // End of actions section. This cannot be confused with action content, because after an
    // action ends the parser expects either an action type code (never zero) or this byte.
    object_utils::push_null(&mut content);

    if let Some(description) = &parcel.description {
        content.extend(description.as_bytes());
    }

    content
}
