//! The wire types of the remote protocol (see `docs/format/REMOTE_PROTOCOL.md`).
//! Shared by the client engine (`util::remote_utils`) and every server implementation
//! (the reference `forklift-server`, and the hosted control plane).

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use crate::util::office_utils::TrustAnchor;

/// The protocol version spoken by this build. A client refuses a remote whose version
/// it does not know; the version only changes when the wire format changes.
pub const PROTOCOL_VERSION: &str = "2026-07-05";

/// The largest number of hashes accepted by one `POST /v1/objects/missing` request;
/// clients batch larger sets.
pub const MAX_MISSING_BATCH: usize = 10_000;

/// The `GET /v1/warehouse` handshake: protocol version, refs and trust in one round trip.
#[derive(Serialize, Deserialize)]
pub struct WarehouseInfo {
    pub protocol: String,

    /// The pallet a franchise (clone) checks out when the user does not choose.
    pub default_pallet: String,

    /// Every pallet with something stacked, mapped to its head parcel hash.
    pub pallets: BTreeMap<String, String>,

    /// The trust anchor, when signing is established on the remote.
    pub trust: Option<TrustAnchorDto>,
}

/// The trust anchor on the wire (the TOML file's fields as JSON). The re-genesis
/// fields (§8.7) are absent for an original enrollment.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct TrustAnchorDto {
    pub genesis: String,
    pub enabled_at: i64,
    pub boundary: Vec<String>,

    /// The genesis of the chain this anchor replaced (re-genesis chain of custody).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_genesis: Option<String>,

    /// The office head of the replaced chain, pinned as attested history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopts: Option<String>,
}

impl From<&TrustAnchor> for TrustAnchorDto {
    fn from(anchor: &TrustAnchor) -> Self {
        TrustAnchorDto {
            genesis: anchor.genesis.clone(),
            enabled_at: anchor.enabled_at,
            boundary: anchor.boundary.clone(),
            prior_genesis: anchor.prior_genesis.clone(),
            adopts: anchor.adopts.clone(),
        }
    }
}

impl TrustAnchorDto {
    /// Convert the wire form back into the local anchor type.
    pub fn to_anchor(&self) -> TrustAnchor {
        TrustAnchor {
            genesis: self.genesis.clone(),
            enabled_at: self.enabled_at,
            boundary: self.boundary.clone(),
            prior_genesis: self.prior_genesis.clone(),
            adopts: self.adopts.clone(),
        }
    }
}

/// The body of `POST /v1/objects/missing`.
#[derive(Serialize, Deserialize)]
pub struct MissingObjectsRequest {
    pub hashes: Vec<String>,
}

/// The response of `POST /v1/objects/missing`: the subset the remote does not have.
#[derive(Serialize, Deserialize)]
pub struct MissingObjectsResponse {
    pub missing: Vec<String>,
}

/// The body of `POST /v1/objects/upload-targets` (additive; a head whose byte plane is
/// object storage). Asks, without sending a single object body, where each of these
/// objects should be uploaded for lift `session`.
#[derive(Serialize, Deserialize)]
pub struct UploadTargetsRequest {
    /// The lift session the uploads belong to; it scopes the staging keys.
    pub session: String,

    pub hashes: Vec<String>,
}

/// The response of `POST /v1/objects/upload-targets`: one verdict per requested hash, so a
/// client learns in a single body-less round trip what to skip, what to send straight to
/// storage, and what to hand the control plane. It subsumes `POST /v1/objects/missing` for
/// the upload path (`present` is the complement of `missing`).
#[derive(Serialize, Deserialize)]
pub struct UploadTargetsResponse {
    /// Objects the remote already has at their canonical key. Do not upload them.
    pub present: Vec<String>,

    /// Presigned `PUT` URLs by hash — upload the bytes straight to storage, bypassing the
    /// control plane. Each URL addresses a *staging* key: the object is not fetchable until
    /// `POST /lift/{session}/commit` (or the staging verifier) promotes it.
    pub targets: BTreeMap<String, String>,

    /// Objects with no presigned target: `PUT` their bytes to `/v1/objects/{hash}` as usual
    /// and the head verifies them inline. A direct head answers with every missing hash here.
    pub direct: Vec<String>,
}

/// The body of `POST /v1/pallets/{name}` — the CAS ref update.
#[derive(Serialize, Deserialize)]
pub struct RefUpdateRequest {
    /// The head the remote is expected to have right now (`None`: the pallet must not
    /// exist yet). A mismatch is a `409` and nothing moves.
    pub old_head: Option<String>,

    /// The parcel the pallet head moves to.
    pub new_head: String,
}

/// The body of `POST /v1/lift/{session}/commit` (additive; the serverless head). After a
/// client has `PUT` its objects straight to storage via presigned staging URLs, it asks the
/// head to verify and promote the session's uploads before the ref update. The head promotes
/// the small `control_plane` objects synchronously and only presence-checks the large `blobs`
/// (the staging verifier promotes those out of band). A direct head verifies every `PUT`
/// inline and never needs this call.
#[derive(Serialize, Deserialize)]
pub struct CommitLiftRequest {
    /// Small objects — parcels, trees, signature sidecars — the head verifies and promotes
    /// synchronously: it reads the staged bytes, checks `Blake3(bytes) == hash`, and only
    /// then copies them to the canonical hash key. A corrupt one refuses the commit.
    pub control_plane: Vec<String>,

    /// Large working blobs, checked for presence at their canonical key only — which is the
    /// proof the staging verifier already hash-checked them. One still in staging simply
    /// reads as not-yet-ready, and the client retries.
    pub blobs: Vec<String>,
}

/// The body of `POST /v1/resolve` — operator identifiers to resolve to display
/// names. Resolution is server-mediated on purpose (DESIGN.html §8.12): the client
/// never talks to the resolution service directly, so the policy that decides *which*
/// names a caller may see is enforced, not advisory. The server answers from its
/// resolution hook (`docs/format/HOOK_PROTOCOL.md`), knowing who is asking.
#[derive(Serialize, Deserialize)]
pub struct ResolveRequest {
    pub identifiers: Vec<String>,
}

/// The response of `POST /v1/resolve`: the names the caller is allowed to see (the
/// policy withholds the rest, and a server with no resolution hook returns none —
/// the client shows pseudonyms either way).
#[derive(Serialize, Deserialize)]
pub struct ResolveResponse {
    pub names: BTreeMap<String, String>,
}

/// The JSON body every error status carries.
#[derive(Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}
