//! In-memory fakes of the two stores, so the whole protocol suite runs in CI against the
//! real [`Head`](crate::Head) logic without AWS. They implement the same semantics the S3
//! and DynamoDB backends must — hash-verified object writes, immutable signatures, an
//! atomic head CAS, a one-way trust door — in a `HashMap` behind a `Mutex`.
//!
//! [`MemoryObjectStore`] can also be put in *redirect mode* to exercise the presigned-URL
//! branch of the head without a real S3, and offers [`MemoryObjectStore::insert_unverified`]
//! to seed the store as if a client had uploaded straight to an S3 staging prefix
//! (bypassing the control plane) — the case the session-commit verification guards.

use std::collections::HashMap;
use std::sync::Mutex;

use forklift_core::model::remote::TrustAnchorDto;
use forklift_core::util::object_utils;
use forklift_core::util::office_utils::TrustAnchor;
use forklift_core::util::pallet_utils::{PalletNamespace, PalletRef, DEFAULT_PALLET_NAME};

use crate::store::{
    CasOutcome, ObjectAccess, ObjectStore, PutOutcome, PutTarget, RefStore, SignatureOutcome,
    TrustOutcome,
};

/// An in-memory [`ObjectStore`]. Object bytes are the uncompressed wire form, keyed by hash.
#[derive(Default)]
pub struct MemoryObjectStore {
    objects: Mutex<HashMap<String, Vec<u8>>>,
    signatures: Mutex<HashMap<String, Vec<u8>>>,
    /// When set, `access`/`put_target` answer with a presigned-style URL under this base
    /// instead of serving bytes directly — the AWS deployment's behaviour.
    redirect_base: Option<String>,
}

impl MemoryObjectStore {
    /// A direct-serving store (the self-host equivalent).
    pub fn new() -> MemoryObjectStore {
        MemoryObjectStore::default()
    }

    /// A store that hands out presigned-style redirect URLs under `base`, so tests can
    /// exercise the head's `307` branch without S3.
    pub fn with_redirect(base: impl Into<String>) -> MemoryObjectStore {
        MemoryObjectStore { redirect_base: Some(base.into()), ..Default::default() }
    }

    /// Seed an object *without* verifying its hash — as if the client had uploaded it
    /// straight to an S3 staging prefix, bypassing the control plane. Used to prove the
    /// session-commit verification rejects a corrupt upload.
    pub fn insert_unverified(&self, hash: &str, bytes: Vec<u8>) {
        self.objects.lock().unwrap().insert(hash.to_string(), bytes);
    }

    /// How many objects are stored (for test assertions).
    pub fn object_count(&self) -> usize {
        self.objects.lock().unwrap().len()
    }
}

impl ObjectStore for MemoryObjectStore {
    fn exists(&self, hash: &str) -> Result<bool, String> {
        Ok(self.objects.lock().unwrap().contains_key(hash))
    }

    fn get(&self, hash: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.objects.lock().unwrap().get(hash).cloned())
    }

    fn put_verified(&self, hash: &str, bytes: &[u8]) -> Result<PutOutcome, String> {
        let actual = object_utils::hash_object_bytes(bytes);

        if actual != hash {
            return Err(format!(
                "Object content does not match its claimed hash {} (actual: {}); refusing to store it.",
                hash, actual
            ));
        }

        let mut objects = self.objects.lock().unwrap();

        if objects.contains_key(hash) {
            return Ok(PutOutcome::AlreadyPresent);
        }

        objects.insert(hash.to_string(), bytes.to_vec());

        Ok(PutOutcome::Created)
    }

    fn get_signature(&self, parcel_hash: &str) -> Result<Option<Vec<u8>>, String> {
        Ok(self.signatures.lock().unwrap().get(parcel_hash).cloned())
    }

    fn put_signature(&self, parcel_hash: &str, bytes: &[u8]) -> Result<SignatureOutcome, String> {
        let mut signatures = self.signatures.lock().unwrap();

        match signatures.get(parcel_hash) {
            Some(existing) if existing == bytes => Ok(SignatureOutcome::AlreadyPresent),
            Some(_) => Ok(SignatureOutcome::Conflict),
            None => {
                signatures.insert(parcel_hash.to_string(), bytes.to_vec());
                Ok(SignatureOutcome::Created)
            }
        }
    }

    fn access(&self, hash: &str) -> Result<Option<ObjectAccess>, String> {
        match &self.redirect_base {
            Some(base) => {
                if self.objects.lock().unwrap().contains_key(hash) {
                    Ok(Some(ObjectAccess::Redirect(format!("{}/objects/{}", base, hash))))
                } else {
                    Ok(None)
                }
            }
            None => Ok(self.get(hash)?.map(ObjectAccess::Direct)),
        }
    }

    fn put_target(&self, hash: &str) -> Result<PutTarget, String> {
        match &self.redirect_base {
            Some(base) => Ok(PutTarget::Redirect(format!("{}/objects/{}", base, hash))),
            None => Ok(PutTarget::Direct),
        }
    }
}

/// An in-memory [`RefStore`]. Pallet heads are keyed by their qualified wire reference
/// (`main`, `@office`), which is unique across the two namespaces.
pub struct MemoryRefStore {
    heads: Mutex<HashMap<String, String>>,
    trust: Mutex<Option<TrustAnchorDto>>,
    default_pallet: String,
}

impl Default for MemoryRefStore {
    fn default() -> MemoryRefStore {
        MemoryRefStore {
            heads: Mutex::new(HashMap::new()),
            trust: Mutex::new(None),
            default_pallet: DEFAULT_PALLET_NAME.to_string(),
        }
    }
}

impl MemoryRefStore {
    /// A fresh ref store with the default pallet (`main`).
    pub fn new() -> MemoryRefStore {
        MemoryRefStore::default()
    }

    fn key(namespace: PalletNamespace, name: &str) -> String {
        PalletRef { namespace, name: name.to_string() }.to_wire()
    }
}

impl RefStore for MemoryRefStore {
    fn get_head(&self, namespace: PalletNamespace, name: &str) -> Result<Option<String>, String> {
        Ok(self.heads.lock().unwrap().get(&Self::key(namespace, name)).cloned())
    }

    fn compare_and_set_head(
        &self,
        namespace: PalletNamespace,
        name: &str,
        expected: Option<&str>,
        new: &str,
    ) -> Result<CasOutcome, String> {
        let mut heads = self.heads.lock().unwrap();
        let key = Self::key(namespace, name);
        let current = heads.get(&key).cloned();

        if current.as_deref() != expected {
            return Ok(CasOutcome::Conflict { current });
        }

        heads.insert(key, new.to_string());

        Ok(CasOutcome::Committed)
    }

    fn list_refs(&self) -> Result<Vec<(PalletRef, String)>, String> {
        self.heads
            .lock()
            .unwrap()
            .iter()
            .map(|(wire, head)| Ok((PalletRef::parse(wire)?, head.clone())))
            .collect()
    }

    fn default_pallet(&self) -> Result<String, String> {
        Ok(self.default_pallet.clone())
    }

    fn get_trust(&self) -> Result<Option<TrustAnchor>, String> {
        Ok(self.trust.lock().unwrap().as_ref().map(|dto| dto.to_anchor()))
    }

    fn put_trust_if_absent(&self, anchor: &TrustAnchor) -> Result<TrustOutcome, String> {
        let incoming = TrustAnchorDto::from(anchor);
        let mut trust = self.trust.lock().unwrap();

        match trust.as_ref() {
            Some(existing) if *existing == incoming => Ok(TrustOutcome::AlreadyIdentical),
            Some(_) => Ok(TrustOutcome::Conflict),
            None => {
                *trust = Some(incoming);
                Ok(TrustOutcome::Established)
            }
        }
    }

    fn replace_trust(&self, anchor: &TrustAnchor) -> Result<(), String> {
        *self.trust.lock().unwrap() = Some(TrustAnchorDto::from(anchor));
        Ok(())
    }
}
