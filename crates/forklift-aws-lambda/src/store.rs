//! The two narrow traits the head's protocol logic runs over.
//!
//! [`ObjectStore`] is the byte plane (canonical objects and signature sidecars); on AWS
//! it is S3, and object bytes reach it directly from the client via presigned URLs. Its
//! reads and existence checks funnel the same access `forklift_core::util::file_utils`
//! makes on a local warehouse (`retrieve_object_by_hash`, `does_object_exist`,
//! `store_object_bytes`); its `access`/`put_target` hooks decide whether the head serves
//! bytes itself (the fake, the self-host equivalent) or hands out a `307` to a storage
//! URL (the S3-backed deployment).
//!
//! [`RefStore`] is the single consistency point — pallet heads and the trust anchor. On
//! AWS it is DynamoDB, and its one non-idempotent operation, [`RefStore::compare_and_set_head`],
//! is a conditional write: exactly the CAS that lets the serverless head scale
//! horizontally where the server head needs an in-process mutex (§4.5, §4.6). Unlike the
//! filesystem, object storage has no directory walk, so ref enumeration is an explicit
//! primitive ([`RefStore::list_refs`]).
//!
//! Every method returns `Result<_, String>`: storage failures are opaque strings that the
//! [`Head`](crate::Head) turns into a `500`. The verification and CAS *semantics* live in
//! the head, not the stores — a store only persists and reports.

use forklift_core::util::office_utils::TrustAnchor;
use forklift_core::util::pallet_utils::{PalletNamespace, PalletRef};

/// The outcome of storing an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    /// The object was newly stored.
    Created,
    /// The object was already present (immutable, so equal hash means equal content).
    AlreadyPresent,
}

/// The outcome of storing a signature sidecar (immutable, like an object, but a
/// *conflicting* sidecar for an already-signed parcel is refused rather than deduplicated).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureOutcome {
    /// The sidecar was newly stored.
    Created,
    /// The identical sidecar was already present.
    AlreadyPresent,
    /// A *different* sidecar already exists — signatures are immutable (`409`).
    Conflict,
}

/// How the head answers a request for an object's bytes.
pub enum ObjectAccess {
    /// The store served the bytes; the head returns them directly (self-host / fake).
    Direct(Vec<u8>),
    /// Follow this URL (a presigned S3 GET) for the bytes — the head answers `307`.
    Redirect(String),
}

/// Where an object upload should go.
pub enum PutTarget {
    /// The head accepts and verifies the bytes itself (self-host / fake).
    Direct,
    /// Upload the bytes to this URL (a presigned S3 PUT); the head answers `307`. The
    /// object becomes fetchable only after it is verified — inline at a completion
    /// callback for small control-plane objects, asynchronously for large blobs
    /// (DESIGN.html §4.2 / §4.6). The verifier reuses the same `Blake3(body) == hash`
    /// check `put_verified` runs.
    Redirect(String),
}

/// The outcome of a compare-and-set on a pallet head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CasOutcome {
    /// The head moved from `expected` to the new value.
    Committed,
    /// The head was not `expected`; nothing moved. Carries the actual current head so
    /// the client can report the divergence.
    Conflict { current: Option<String> },
}

/// The outcome of establishing the trust anchor (a one-way door, §4.4 / §8.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustOutcome {
    /// No anchor existed; this one was planted.
    Established,
    /// The identical anchor was already present (idempotent).
    AlreadyIdentical,
    /// A *different* anchor already exists; trust cannot be replaced silently (`409`).
    /// The one sanctioned replacement, a re-genesis, goes through
    /// [`RefStore::replace_trust`].
    Conflict,
}

/// The byte plane: canonical objects and signature sidecars, addressed by hash.
///
/// Object bytes are the *uncompressed* wire form — the verifiable form the hash covers
/// (`REMOTE_PROTOCOL.md` invariant 3). What the backend does at rest (the fake keeps
/// bytes as-is; a filesystem mirror re-compresses via `store_object_bytes`; S3 stores
/// what it is given) is its own business.
pub trait ObjectStore {
    /// Whether an object is present at its hash. On S3 this is a `HEAD`; it is the seam
    /// the closure walk uses for the (large, many) working blobs it never reads.
    fn exists(&self, hash: &str) -> Result<bool, String>;

    /// The uncompressed object bytes, or `None` if absent.
    fn get(&self, hash: &str) -> Result<Option<Vec<u8>>, String>;

    /// Verify `Blake3(bytes) == hash` and store the object; nothing unverified may ever
    /// become fetchable (invariant 1). Storing an already-present hash is a no-op.
    fn put_verified(&self, hash: &str, bytes: &[u8]) -> Result<PutOutcome, String>;

    /// A parcel's signature sidecar bytes, or `None` for an unsigned parcel.
    fn get_signature(&self, parcel_hash: &str) -> Result<Option<Vec<u8>>, String>;

    /// Store a signature sidecar. The bytes are assumed already structurally validated by
    /// the caller (the head, via `sign_utils::validate_raw_parcel_signature`). Immutable:
    /// identical re-store is a no-op, a conflicting one is refused.
    fn put_signature(&self, parcel_hash: &str, bytes: &[u8]) -> Result<SignatureOutcome, String>;

    /// How to answer a byte read for `hash`. The default serves the bytes directly (the
    /// self-host / in-memory behaviour); an S3-backed store overrides this to answer with
    /// a presigned-GET [`ObjectAccess::Redirect`].
    fn access(&self, hash: &str) -> Result<Option<ObjectAccess>, String> {
        Ok(self.get(hash)?.map(ObjectAccess::Direct))
    }

    /// Where an upload of `hash` should go. The default accepts the bytes directly; an
    /// S3-backed store overrides this to hand out a presigned-PUT [`PutTarget::Redirect`].
    fn put_target(&self, _hash: &str) -> Result<PutTarget, String> {
        Ok(PutTarget::Direct)
    }
}

/// The consistency point: pallet heads and the trust anchor.
pub trait RefStore {
    /// The current head of a pallet, or `None` if it is unborn.
    fn get_head(&self, namespace: PalletNamespace, name: &str) -> Result<Option<String>, String>;

    /// Atomically move a pallet head from `expected` to `new` (a DynamoDB conditional
    /// write). `expected: None` means "the pallet must not exist yet". A mismatch commits
    /// nothing and reports the actual head — the CAS that catches concurrent lifts.
    fn compare_and_set_head(
        &self,
        namespace: PalletNamespace,
        name: &str,
        expected: Option<&str>,
        new: &str,
    ) -> Result<CasOutcome, String>;

    /// Every pallet with something stacked, across both namespaces. Explicit because
    /// object storage has no directory walk (unlike `pallet_utils::all_pallet_refs`).
    fn list_refs(&self) -> Result<Vec<(PalletRef, String)>, String>;

    /// The pallet a franchise checks out when the user does not choose (git's `HEAD`).
    fn default_pallet(&self) -> Result<String, String>;

    /// The warehouse's trust anchor, or `None` before trust is established.
    fn get_trust(&self) -> Result<Option<TrustAnchor>, String>;

    /// Plant the trust anchor iff none exists yet (the one-way door). Idempotent for an
    /// identical anchor; a conflicting one is refused. Atomic, like the head CAS.
    fn put_trust_if_absent(&self, anchor: &TrustAnchor) -> Result<TrustOutcome, String>;

    /// Replace the trust anchor — the one sanctioned overwrite, a re-genesis (§8.7). The
    /// head validates the chain-of-custody before calling this.
    fn replace_trust(&self, anchor: &TrustAnchor) -> Result<(), String>;
}
