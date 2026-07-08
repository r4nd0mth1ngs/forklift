use serde::Serialize;

/// An operator is a person or an entity that interacts with the system.
///
/// The `identifier` is the operator's on-chain identity — an opaque string as far as
/// the chain is concerned. When none is configured, Forklift mints a UUID, so chains
/// are pseudonymous by default (zero PII in signed history); a hosting provider
/// supplies its own minted id, and a team that wants human-readable chains may set
/// any string, accepting that it is public in every clone, forever. The `name` is
/// local display data and is never stored on-chain.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Operator {
    /// The display name of the operator. Local configuration — never stored on-chain;
    /// falls back to the identifier when unset.
    pub name: String,

    /// The operator id recorded in parcels and office records. Opaque to the chain;
    /// a minted UUID by default (see `config_utils`).
    pub identifier: String,
}
