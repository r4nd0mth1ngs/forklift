use serde::Serialize;
use crate::model::parcel_action::ParcelAction;

/// A parcel is a set of changes.
/// It points to a tree which is a snapshot of the warehouse.
/// It also includes some metadata, like the parent parcel, and the operators
/// who contributed to the changes.
#[derive(Serialize)]
pub struct Parcel {
    pub tree_hash: String,
    pub parents: Vec<String>,
    pub actions: Vec<ParcelAction>,
    pub description: Option<String>,
}