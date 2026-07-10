//! [`S3ObjectStore`]: the byte plane on `aws-sdk-s3`.
//!
//! # Key layout
//!
//! One bucket, four prefixes, and the boundary between them *is* invariant 1:
//!
//! * `objects/{hash}` — the **canonical** namespace. Everything here is hash-verified, and
//!   the only two write paths into it are [`put_verified`](S3ObjectStore::put_verified)
//!   (which hashes the bytes first) and [`verify_and_promote`](S3ObjectStore::verify_and_promote)
//!   (which hashes the staged bytes first). Nothing else can put a key here — in particular,
//!   no presigned `PUT` ever addresses this prefix.
//! * `staging/{session}/{hash}` — where a client's presigned `PUT` lands. Bytes here are
//!   invisible to [`exists`](S3ObjectStore::exists)/[`get`](S3ObjectStore::get); they become
//!   fetchable only when `verify_and_promote` copies them to `objects/{hash}` after checking
//!   the hash. This is the whole reason a `PUT` target needs a session.
//! * `signatures/{parcel_hash}` — signature sidecars, immutable like objects. A distinct
//!   prefix, never `objects/`, because a parcel's sidecar shares the parcel's hash and must
//!   not collide with the parcel object.
//! * `responses/{content_hash}` — offloaded `batch` bundles, ephemeral and content-addressed,
//!   deliberately outside `objects/` so nothing here is reachable as an object (invariant 1
//!   is not in play; the client verifies every record on import regardless).
//!
//! # The conditional-write CAS
//!
//! S3 has no lock, so the atomicity the fake gets from its `objects` mutex comes here from an
//! `If-None-Match: *` conditional `PUT`: the write succeeds only if the key does not yet
//! exist, else S3 answers `412`. That single primitive gives `put_verified` its
//! `Created`/`AlreadyPresent` split and lets two promoters race onto one canonical key with
//! exactly one winner — the atomicity the fake's module docs say "an S3 + DynamoDB backend
//! owes."
//!
//! # Presigned URLs
//!
//! Reads and staged writes are handed to the client as presigned URLs (a `307` from the
//! head), so object bytes never traverse the control plane. Every presigned URL is valid for
//! [`PRESIGN_TTL`] — short, constant, and the same for reads and writes. A presigned `PUT` is
//! constructible by exactly one private helper, [`S3ObjectStore::presign_staging_put`], which
//! hardcodes the `staging/` prefix; there is no code path that presigns a `PUT` to a hash key.

use std::time::Duration;

use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;

use forklift_core::util::object_utils;

use crate::aws::sdk::{describe, is_not_found, is_precondition_failed};
use crate::blocking::AsyncBridge;
use crate::store::{ObjectAccess, ObjectStore, PromoteOutcome, PutOutcome, PutTarget, SignatureOutcome};

/// How long a presigned URL is valid. Short and constant, the same for a read and a staged
/// write: long enough for a client to upload or download one object over a slow link, short
/// enough that a leaked URL is a small window. 15 minutes matches the SDK's own default and
/// the hosted deployment's assumption of a live, actively-lifting client.
pub const PRESIGN_TTL: Duration = Duration::from_secs(15 * 60);

/// The canonical key of an object — the only prefix `exists`/`get`/`access` read, and the
/// only prefix hash-verified writes target.
fn object_key(hash: &str) -> String {
    format!("objects/{}", hash)
}

/// The staging key an upload for `hash` lands at under lift `session`. Never `objects/{hash}`:
/// bytes here are unverified and unfetchable until promotion (invariant 1).
fn staging_key(session: &str, hash: &str) -> String {
    format!("staging/{}/{}", session, hash)
}

/// The prefix sweeping a whole session's staged uploads.
fn staging_prefix(session: &str) -> String {
    format!("staging/{}/", session)
}

/// A parcel's signature-sidecar key. A distinct prefix so it never collides with the parcel
/// object at `objects/{parcel_hash}`.
fn signature_key(parcel_hash: &str) -> String {
    format!("signatures/{}", parcel_hash)
}

/// The ephemeral, content-addressed key of an offloaded response body. Outside `objects/`, so
/// nothing here is ever an object at a hash key.
fn response_key(content_hash: &str) -> String {
    format!("responses/{}", content_hash)
}

/// The S3-backed [`ObjectStore`]: the byte plane of an AWS serverless head.
///
/// Every method is synchronous (the settled seam, R4) and drives the async SDK through the
/// [`AsyncBridge`]. It must be built inside a multi-thread runtime and its methods called
/// from a blocking thread, exactly as the `Head` contract requires.
pub struct S3ObjectStore {
    client: aws_sdk_s3::Client,
    bucket: String,
    bridge: AsyncBridge,
}

/// The outcome of a conditional (`If-None-Match: *`) write: it landed, or the key already
/// existed. Every other failure is a genuine error.
enum Conditional {
    /// The write succeeded — the key was free and now holds these bytes.
    Written,
    /// The key already existed; nothing was written (`412`).
    AlreadyExists,
}

impl S3ObjectStore {
    /// Build the store over an S3 `client` addressing `bucket`, driving its async calls
    /// through `bridge`. Capture the bridge on the runtime thread (see `aws::config`).
    pub fn new(client: aws_sdk_s3::Client, bucket: String, bridge: AsyncBridge) -> S3ObjectStore {
        S3ObjectStore { client, bucket, bridge }
    }

    /// Whether a key exists (an S3 `HEAD`). A `404` is `Ok(false)`, not an error.
    async fn key_exists(&self, key: &str) -> Result<bool, String> {
        match self.client.head_object().bucket(&self.bucket).key(key).send().await {
            Ok(_) => Ok(true),
            Err(err) if is_not_found(&err) => Ok(false),
            Err(err) => Err(describe("S3 head_object", err)),
        }
    }

    /// The bytes at a key, or `None` when it is absent (a `404` is `Ok(None)`).
    async fn key_bytes(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        match self.client.get_object().bucket(&self.bucket).key(key).send().await {
            Ok(output) => {
                let bytes = output
                    .body
                    .collect()
                    .await
                    .map_err(|err| format!("S3 read of {} failed: {}", key, err))?;

                Ok(Some(bytes.to_vec()))
            }
            Err(err) if is_not_found(&err) => Ok(None),
            Err(err) => Err(describe("S3 get_object", err)),
        }
    }

    /// A conditional `PUT`: write `bytes` at `key` only if the key does not already exist.
    /// The `If-None-Match: *` header is the byte plane's CAS — it is how immutability and
    /// racing-promoter serialization are enforced without a lock.
    async fn put_if_absent(&self, key: &str, bytes: Vec<u8>) -> Result<Conditional, String> {
        match self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .if_none_match("*")
            .body(ByteStream::from(bytes))
            .send()
            .await
        {
            Ok(_) => Ok(Conditional::Written),
            Err(err) if is_precondition_failed(&err) => Ok(Conditional::AlreadyExists),
            Err(err) => Err(describe("S3 put_object", err)),
        }
    }

    /// An unconditional `PUT` — for the ephemeral response prefix, where a repeated offload of
    /// the same content-addressed bundle harmlessly overwrites itself.
    async fn put_overwrite(&self, key: &str, bytes: Vec<u8>) -> Result<(), String> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map(|_| ())
            .map_err(|err| describe("S3 put_object", err))
    }

    /// Best-effort delete of a staged key. A failure here never fails the caller: a leftover
    /// staged object is unfetchable (it is not at a hash key) and is swept by
    /// [`discard_session`](S3ObjectStore::discard_session) later.
    async fn drop_staged(&self, key: &str) {
        let _ = self.client.delete_object().bucket(&self.bucket).key(key).send().await;
    }

    /// Presign a `GET` for `key`, valid for [`PRESIGN_TTL`].
    async fn presign_get(&self, key: &str) -> Result<String, String> {
        let config = PresigningConfig::expires_in(PRESIGN_TTL)
            .map_err(|err| format!("building the presigning config failed: {}", err))?;

        let request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(config)
            .await
            .map_err(|err| describe("S3 presign get_object", err))?;

        Ok(request.uri().to_string())
    }

    /// Presign a `PUT` into the session's **staging** prefix, valid for [`PRESIGN_TTL`]. This
    /// is the *only* presigned-`PUT` path in the store, and it hardcodes `staging/{session}/`:
    /// there is no way to hand a client a presigned `PUT` to a canonical hash key. That is the
    /// structural half of invariant 1.
    async fn presign_staging_put(&self, session: &str, hash: &str) -> Result<String, String> {
        let config = PresigningConfig::expires_in(PRESIGN_TTL)
            .map_err(|err| format!("building the presigning config failed: {}", err))?;

        let request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(staging_key(session, hash))
            .presigned(config)
            .await
            .map_err(|err| describe("S3 presign put_object", err))?;

        Ok(request.uri().to_string())
    }
}

impl ObjectStore for S3ObjectStore {
    fn exists(&self, hash: &str) -> Result<bool, String> {
        self.bridge.block_on(self.key_exists(&object_key(hash)))
    }

    fn get(&self, hash: &str) -> Result<Option<Vec<u8>>, String> {
        self.bridge.block_on(self.key_bytes(&object_key(hash)))
    }

    fn put_verified(&self, hash: &str, bytes: &[u8]) -> Result<PutOutcome, String> {
        // Verify before the network call: nothing unverified is ever handed to the canonical
        // namespace, and the message matches the fake so the head's `422` reads identically.
        let actual = object_utils::hash_object_bytes(bytes);

        if actual != hash {
            return Err(format!(
                "Object content does not match its claimed hash {} (actual: {}); refusing to store it.",
                hash, actual
            ));
        }

        self.bridge.block_on(async {
            match self.put_if_absent(&object_key(hash), bytes.to_vec()).await? {
                Conditional::Written => Ok(PutOutcome::Created),
                Conditional::AlreadyExists => Ok(PutOutcome::AlreadyPresent),
            }
        })
    }

    fn get_signature(&self, parcel_hash: &str) -> Result<Option<Vec<u8>>, String> {
        self.bridge.block_on(self.key_bytes(&signature_key(parcel_hash)))
    }

    fn put_signature(&self, parcel_hash: &str, bytes: &[u8]) -> Result<SignatureOutcome, String> {
        let key = signature_key(parcel_hash);

        self.bridge.block_on(async {
            match self.put_if_absent(&key, bytes.to_vec()).await? {
                Conditional::Written => Ok(SignatureOutcome::Created),
                // A sidecar already exists. Immutable: identical bytes are a no-op, different
                // bytes are refused — so read the incumbent and compare, exactly as the fake.
                Conditional::AlreadyExists => match self.key_bytes(&key).await? {
                    Some(existing) if existing == bytes => Ok(SignatureOutcome::AlreadyPresent),
                    Some(_) => Ok(SignatureOutcome::Conflict),
                    // Raced with a delete that cannot happen (sidecars are never deleted); treat
                    // the vanished incumbent as absent-and-now-different rather than claim it.
                    None => Ok(SignatureOutcome::Conflict),
                },
            }
        })
    }

    fn access(&self, hash: &str) -> Result<Option<ObjectAccess>, String> {
        let key = object_key(hash);

        self.bridge.block_on(async {
            // Mirror the fake: absent is `None` (the head's `404`), present is a presigned GET.
            // The HEAD before the presign keeps a redirect from ever pointing at a missing key.
            if !self.key_exists(&key).await? {
                return Ok(None);
            }

            let url = self.presign_get(&key).await?;

            Ok(Some(ObjectAccess::Redirect(url)))
        })
    }

    fn put_target(&self, session: Option<&str>, hash: &str) -> Result<PutTarget, String> {
        // The S3 head always stages: the bytes go straight to storage and are promoted at
        // commit. A session-less upload has nowhere to be promoted from, so it is refused —
        // never by handing out a presigned PUT to the hash key (the invariant-1 hole).
        match session {
            Some(session) => {
                let url = self.bridge.block_on(self.presign_staging_put(session, hash))?;

                Ok(PutTarget::Staged(url))
            }
            None => Ok(PutTarget::SessionRequired),
        }
    }

    /// Take the staged bytes (so a corrupt upload is *discarded* by the same act that
    /// rejects it), and promote them only if they hash to `hash`.
    fn verify_and_promote(&self, session: &str, hash: &str) -> Result<PromoteOutcome, String> {
        let canonical = object_key(hash);
        let staged = staging_key(session, hash);

        self.bridge.block_on(async {
            // 1. Already canonical? Mirror the fake's first check: the object was verified once
            //    and is immutable, so sweep the now-redundant staged copy and report it. This
            //    is also the cheap idempotent-retry path — one HEAD, no download.
            if self.key_exists(&canonical).await? {
                self.drop_staged(&staged).await;

                return Ok(PromoteOutcome::AlreadyPresent);
            }

            // 2. Take the staged bytes. Absent means either "never uploaded" or "a racing
            //    promoter already promoted and swept it" — re-check the canonical key to tell
            //    them apart, so the loser of a race never spuriously reports `Missing`.
            let Some(bytes) = self.key_bytes(&staged).await? else {
                if self.key_exists(&canonical).await? {
                    return Ok(PromoteOutcome::AlreadyPresent);
                }

                return Ok(PromoteOutcome::Missing);
            };

            // 3. Verify. Corrupt bytes are discarded by the same act that rejects them, and
            //    never reach the canonical key — invariant 1 holds against a client that PUT
            //    garbage straight to the staging URL.
            let actual = object_utils::hash_object_bytes(&bytes);

            if actual != hash {
                self.drop_staged(&staged).await;

                return Ok(PromoteOutcome::Corrupt { actual });
            }

            // 4. Promote with the conditional write. If a racing promoter beat us to the
            //    canonical key, the `If-None-Match` fails and we report `AlreadyPresent` — the
            //    same serialization the fake gets from its lock, so exactly one promoter wins.
            match self.put_if_absent(&canonical, bytes).await? {
                Conditional::Written => {
                    self.drop_staged(&staged).await;

                    Ok(PromoteOutcome::Promoted)
                }
                Conditional::AlreadyExists => {
                    self.drop_staged(&staged).await;

                    Ok(PromoteOutcome::AlreadyPresent)
                }
            }
        })
    }

    fn discard_session(&self, session: &str) -> Result<(), String> {
        let prefix = staging_prefix(session);

        self.bridge.block_on(async {
            let mut continuation: Option<String> = None;

            loop {
                let mut request =
                    self.client.list_objects_v2().bucket(&self.bucket).prefix(&prefix);

                if let Some(token) = continuation.take() {
                    request = request.continuation_token(token);
                }

                let page = request
                    .send()
                    .await
                    .map_err(|err| describe("S3 list_objects_v2", err))?;

                for object in page.contents() {
                    if let Some(key) = object.key() {
                        self.client
                            .delete_object()
                            .bucket(&self.bucket)
                            .key(key)
                            .send()
                            .await
                            .map_err(|err| describe("S3 delete_object", err))?;
                    }
                }

                match page.next_continuation_token() {
                    Some(token) => continuation = Some(token.to_string()),
                    None => break,
                }
            }

            Ok(())
        })
    }

    fn offload_response(&self, bytes: &[u8]) -> Result<Option<String>, String> {
        // A content-addressed key in the ephemeral response prefix — never an object. Two
        // identical bundles land on the same key, so the write is an idempotent overwrite.
        let key = response_key(&object_utils::hash_object_bytes(bytes));

        self.bridge.block_on(async {
            self.put_overwrite(&key, bytes.to_vec()).await?;

            let url = self.presign_get(&key).await?;

            Ok(Some(url))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_canonical_and_staging_prefixes_never_collide() {
        let hash = "a".repeat(64);

        assert_eq!(object_key(&hash), format!("objects/{}", hash));
        assert_eq!(staging_key("lift-1", &hash), format!("staging/lift-1/{}", hash));

        // The one property invariant 1 rests on at the key layout: a staged key is never the
        // canonical key, so bytes uploaded to it are not fetchable at the hash.
        assert_ne!(object_key(&hash), staging_key("lift-1", &hash));
        assert!(!staging_key("lift-1", &hash).starts_with("objects/"));
    }

    #[test]
    fn signature_and_response_prefixes_are_outside_the_object_namespace() {
        let hash = "b".repeat(64);

        // A sidecar shares the parcel's hash but must not collide with the parcel object.
        assert_eq!(signature_key(&hash), format!("signatures/{}", hash));
        assert_ne!(signature_key(&hash), object_key(&hash));

        // A response body is never reachable as an object at a hash key.
        assert_eq!(response_key(&hash), format!("responses/{}", hash));
        assert!(!response_key(&hash).starts_with("objects/"));
    }

    #[test]
    fn a_session_prefix_bounds_exactly_its_staged_keys() {
        let prefix = staging_prefix("lift-7");

        assert_eq!(prefix, "staging/lift-7/");
        assert!(staging_key("lift-7", &"c".repeat(64)).starts_with(&prefix));
        // The prefix of one session never captures another's keys.
        assert!(!staging_key("lift-8", &"c".repeat(64)).starts_with(&prefix));
    }

    #[test]
    fn the_presign_ttl_is_short_and_constant() {
        assert_eq!(PRESIGN_TTL, Duration::from_secs(900));
    }
}
