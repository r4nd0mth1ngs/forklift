use chrono::{DateTime, Utc};
use chrono::serde::ts_seconds;
use serde::Serialize;
use crate::enums::parcel_action_type::ParcelActionType;
use crate::model::operator::Operator;

/// An interaction between an operator and a parcel.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParcelAction {
    pub operator: Operator,
    pub action: ParcelActionType,
    pub description: Option<String>,
    #[serde(with = "ts_seconds")]
    pub timestamp: DateTime<Utc>,
}