//! The verification bridge: reuse `forklift_core`'s filesystem-based audit against an
//! [`ObjectStore`](crate::store::ObjectStore) by mirroring the objects the audit must
//! *read* into a throwaway on-disk `.forklift`.
//!
//! Verification never reads a working-directory blob — it only checks that one exists
//! (the single `does_object_exist` call in `verify_parcel_closure`). Everything it *reads*
//! (parcels, trees, office-record blobs, signature sidecars) is small. So the AWS head
//! mirrors exactly those small objects into a scratch warehouse, enters its storage-root
//! scope, and runs the exact same `audit_utils` code the CLI and server head run; the
//! working blobs stay in object storage and their presence is checked with an S3 `HEAD`
//! via `verify_parcel_closure_with` (DESIGN.html §4.6). In a warm Lambda container the
//! scratch persists across invocations, amortizing the mirror.
//!
//! The scratch is a real filesystem directory precisely so the reuse is total: the same
//! bytes, the same parser, the same signature and privilege checks — no second
//! implementation of the security-critical path to drift out of sync.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use forklift_core::globals::StorageRootScope;
use forklift_core::util::{object_utils, sign_utils, warehouse_utils};

use crate::store::ObjectStore;

/// Monotonic suffix so concurrent scratches on one host never collide.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A throwaway on-disk `.forklift` warehouse used to run `forklift_core`'s audit against
/// objects that live in an [`ObjectStore`]. Removed on drop.
pub struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    /// Create and prepare a fresh scratch warehouse.
    pub fn new() -> Result<Scratch, String> {
        let unique = format!(
            "forklift-head-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );

        let dir = std::env::temp_dir().join(unique);

        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Error while creating the scratch warehouse: {}", e))?;

        let scratch = Scratch { dir };

        // Lay down `.forklift/…` so the storage layer finds a real warehouse.
        scratch.scoped(|| warehouse_utils::prepare_warehouse().map(|_| ()))?;

        Ok(scratch)
    }

    /// Run `work` under this scratch's storage-root scope, so every `forklift_core`
    /// storage path resolves inside it. Strictly synchronous (the scope is thread-local
    /// and must never cross an `.await`). Generic over the error type so a caller can run
    /// the audit — which distinguishes a conflict from a verification failure — inside the
    /// same scope as the mirror.
    pub fn scoped<T, E>(&self, work: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
        let _scope = StorageRootScope::enter(&self.dir);

        work()
    }

    /// The scratch warehouse root.
    pub fn root(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Mirror into the current scratch (the caller must already be inside [`Scratch::scoped`])
/// every object the audit reads while walking the closure of `head`: the parcels, their
/// trees and — when `include_file_blobs` is set — the file-content blobs too, plus each
/// parcel's signature sidecar.
///
/// `include_file_blobs` is `true` for a meta pallet (the office), whose "blobs" are the
/// tracked-metadata records that `verify_office_chain` reads; it is `false` for a working
/// pallet, whose blobs are file content the audit only existence-checks — those stay in
/// the object store and are answered by [`ObjectStore::exists`].
///
/// Objects the store lacks are skipped rather than erroring: the subsequent
/// `verify_parcel_closure` produces the authoritative "history incomplete" diagnostic.
/// `mirrored` dedups across several `materialize` calls in one ref update (the office
/// chain and the target pallet share history).
pub fn materialize(
    store: &dyn ObjectStore,
    head: &str,
    include_file_blobs: bool,
    mirrored: &mut HashSet<String>,
) -> Result<(), String> {
    let mut queue: Vec<String> = vec![head.to_string()];

    while let Some(hash) = queue.pop() {
        if !mirrored.insert(hash.clone()) {
            continue;
        }

        let Some(bytes) = store.get(&hash)? else {
            // Absent: leave the gap for `verify_parcel_closure` to report precisely.
            continue;
        };

        object_utils::store_object_bytes(&hash, &bytes)?;

        if let Some(sidecar) = store.get_signature(&hash)? {
            sign_utils::store_raw_parcel_signature(&hash, &sidecar)?;
        }

        // The head and every parent are parcels; a parse failure (never expected) simply
        // stops this branch and lets verification report the corruption.
        let Ok(parcel) = object_utils::load_parcel(&hash) else {
            continue;
        };

        materialize_tree(store, &parcel.tree_hash, include_file_blobs, mirrored)?;

        queue.extend(parcel.parents);
    }

    Ok(())
}

/// Mirror a tree and everything below it that the audit reads: the tree objects always,
/// the file blobs only when `include_file_blobs` (see [`materialize`]).
fn materialize_tree(
    store: &dyn ObjectStore,
    tree_hash: &str,
    include_file_blobs: bool,
    mirrored: &mut HashSet<String>,
) -> Result<(), String> {
    if !mirrored.insert(tree_hash.to_string()) {
        return Ok(());
    }

    let Some(bytes) = store.get(tree_hash)? else {
        return Ok(());
    };

    object_utils::store_object_bytes(tree_hash, &bytes)?;

    let tree = object_utils::load_tree(tree_hash)?;

    if include_file_blobs {
        for (_, file) in tree.get_files() {
            if mirrored.insert(file.hash.clone()) {
                if let Some(blob) = store.get(&file.hash)? {
                    object_utils::store_object_bytes(&file.hash, &blob)?;
                }
            }
        }
    }

    for (_, subtree) in tree.get_subtrees() {
        materialize_tree(store, &subtree.hash, include_file_blobs, mirrored)?;
    }

    Ok(())
}
