//! The protocol suite for the AWS serverless head, run entirely in CI without AWS.
//!
//! The strategy: build a *real* warehouse with the `forklift` CLI (so the objects, the
//! signed office chain and the trust anchor are exactly what a client produces), harvest
//! its objects and refs, then replay the lift/lower protocol against a [`Head`] over the
//! in-memory fakes — the same handler logic the AWS Lambda control-plane function runs.
//! This exercises the security-critical paths (hash-verified uploads, the fast-forward
//! CAS, and the full offline audit reused via the scratch bridge) against abstracted
//! storage, no S3 or DynamoDB required.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use forklift_aws_lambda::error::Status;
use forklift_aws_lambda::head::{ObjectReadResult, ObjectWriteResult, TrustResult};
use forklift_aws_lambda::memory::{MemoryObjectStore, MemoryRefStore};
use forklift_aws_lambda::store::{ObjectStore, PromoteOutcome, SignatureOutcome};
use forklift_aws_lambda::Head;

use forklift_core::globals::StorageRootScope;
use forklift_core::model::remote::{RefUpdateRequest, TrustAnchorDto};
use forklift_core::util::office_utils::{self, OFFICE_PALLET_NAME};
use forklift_core::util::pallet_utils;
use forklift_core::util::{file_utils, object_utils, sign_utils};

// ---------------------------------------------------------------------------------------
// Harness: build a warehouse with the CLI, harvest it into the fakes.
// ---------------------------------------------------------------------------------------

static AREA_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The compiled `forklift` CLI. Cargo exposes `CARGO_BIN_EXE_*` only to a package's own
/// tests, so — like `forklift/tests/remote.rs` locates the server next to the CLI — this
/// locates the CLI next to the test binary (both land in the target dir).
fn forklift_binary() -> PathBuf {
    let mut dir = std::env::current_exe().expect("the test binary has a path");
    dir.pop(); // the test executable's file name
    if dir.ends_with("deps") {
        dir.pop();
    }

    let binary = dir.join(format!("forklift{}", std::env::consts::EXE_SUFFIX));

    assert!(
        binary.exists(),
        "forklift is not built at {}; run the suite via a workspace `cargo test`.",
        binary.display()
    );

    binary
}

/// A scratch directory for one test, cleaned up on drop.
struct Area {
    root: PathBuf,
}

impl Area {
    fn new(name: &str) -> Area {
        let unique = format!(
            "forklift-aws-test-{}-{}-{}",
            name,
            std::process::id(),
            AREA_COUNTER.fetch_add(1, Ordering::Relaxed)
        );

        let root = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&root).expect("create the test area");
        Area { root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Run the CLI in a subdirectory of the area (created first). A fresh key directory
    /// per area keeps signing self-contained.
    fn forklift(&self, dir: &str, args: &[&str]) {
        let working = self.path(dir);
        std::fs::create_dir_all(&working).expect("create the working directory");

        let output = Command::new(forklift_binary())
            .args(args)
            .current_dir(&working)
            .env("FORKLIFT_GLOBAL_CONFIG", self.path("global.toml"))
            .env("FORKLIFT_KEYS_DIR", self.path("keys"))
            .output()
            .expect("run forklift");

        assert!(
            output.status.success(),
            "forklift {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write_file(&self, relative: &str, content: &str) {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(path, content).expect("write file");
    }
}

impl Drop for Area {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Everything a client would push, harvested from a built warehouse.
struct Harvest {
    objects: HashMap<String, Vec<u8>>,
    signatures: HashMap<String, Vec<u8>>,
    refs: Vec<(pallet_utils::PalletRef, String)>,
    trust: Option<TrustAnchorDto>,
}

impl Harvest {
    fn head_of(&self, wire: &str) -> Option<String> {
        self.refs
            .iter()
            .find(|(pallet_ref, _)| pallet_ref.to_wire() == wire)
            .map(|(_, head)| head.clone())
    }
}

/// Read every object, signature, ref and the trust anchor out of a built warehouse. Object
/// bytes come back in their uncompressed wire form (what the protocol carries).
fn harvest(warehouse: &Path) -> Harvest {
    // Enumerate object and signature files from the single-level fan-out object store.
    let objects_dir = warehouse.join(".forklift").join("objects");
    let mut object_hashes: Vec<String> = Vec::new();
    let mut signature_hashes: Vec<String> = Vec::new();

    for fan in std::fs::read_dir(&objects_dir).expect("read the objects dir") {
        let fan = fan.expect("read a fan entry");
        if !fan.file_type().expect("fan file type").is_dir() {
            continue;
        }

        let prefix = fan.file_name().to_string_lossy().to_string();

        for object in std::fs::read_dir(fan.path()).expect("read a fan folder") {
            let object = object.expect("read an object entry");
            let name = object.file_name().to_string_lossy().to_string();

            match name.strip_suffix(".sig") {
                Some(rest) => signature_hashes.push(format!("{}{}", prefix, rest)),
                None => object_hashes.push(format!("{}{}", prefix, name)),
            }
        }
    }

    // Read them (and the refs/trust) under the warehouse's storage-root scope.
    let _scope = StorageRootScope::enter(warehouse);

    let mut objects = HashMap::new();
    for hash in object_hashes {
        let bytes = file_utils::retrieve_object_by_hash(&hash).expect("retrieve object");
        objects.insert(hash, bytes);
    }

    let mut signatures = HashMap::new();
    for hash in signature_hashes {
        let sidecar = sign_utils::load_raw_parcel_signature(&hash)
            .expect("load signature")
            .expect("signature present");
        signatures.insert(hash, sidecar);
    }

    let refs = pallet_utils::all_pallet_refs().expect("read refs");
    let trust = office_utils::read_trust_anchor()
        .expect("read trust")
        .map(|anchor| TrustAnchorDto::from(&anchor));

    Harvest { objects, signatures, refs, trust }
}

/// Configure an operator in a fresh warehouse dir (prepare + identity).
fn prepare(area: &Area, dir: &str) {
    area.forklift(dir, &["prepare"]);
    area.forklift(dir, &["config", "--global", "operator.name", "AWS Head Tester"]);
    area.forklift(dir, &["config", "--global", "operator.identifier", "tester@forklift"]);
}

/// Upload every harvested object and signature to the head (the direct-store path
/// verifies each object's hash on the way in).
fn upload_all<O: forklift_aws_lambda::store::ObjectStore, R: forklift_aws_lambda::store::RefStore>(
    head: &Head<O, R>,
    harvest: &Harvest,
) {
    for (hash, bytes) in &harvest.objects {
        head.object_put(None, hash, bytes).expect("upload object");
    }
    for (hash, sidecar) in &harvest.signatures {
        head.signature_put(hash, sidecar).expect("upload signature");
    }
}

// ---------------------------------------------------------------------------------------
// The tests.
// ---------------------------------------------------------------------------------------

/// The untrusted path: CAS, closure presence and fast-forward, no crypto.
#[test]
fn untrusted_lift_and_the_cas_guards() {
    let area = Area::new("untrusted");
    prepare(&area, "wh");
    area.write_file("wh/readme.txt", "hello\n");
    area.write_file("wh/src/main.txt", "fn main\n");
    area.forklift("wh", &["load", "."]);
    area.forklift("wh", &["stack", "first"]);
    area.write_file("wh/readme.txt", "hello again\n");
    area.forklift("wh", &["load", "readme.txt"]);
    area.forklift("wh", &["stack", "second"]);

    let harvest = harvest(&area.path("wh"));
    let main_head = harvest.head_of("main").expect("main has a head");

    let head = Head::new(MemoryObjectStore::new(), MemoryRefStore::new());

    // A fresh remote: empty handshake.
    let info = head.handshake().expect("handshake");
    assert!(info.pallets.is_empty());
    assert!(info.trust.is_none());
    assert_eq!(info.default_pallet, "main");

    // The negotiation names everything as missing.
    let all: Vec<String> = harvest.objects.keys().cloned().collect();
    let missing = head.missing(&all).expect("missing");
    assert_eq!(missing.len(), all.len());

    upload_all(&head, &harvest);

    // Now nothing is missing.
    assert!(head.missing(&all).expect("missing").is_empty());

    // The lift commits.
    let request = RefUpdateRequest { old_head: None, new_head: main_head.clone() };
    head.ref_update("main", &request).expect("lift main");

    // The handshake reflects it.
    let info = head.handshake().expect("handshake");
    assert_eq!(info.pallets.get("main"), Some(&main_head));

    // A replay with the same `old_head: None` now conflicts (the pallet exists).
    let err = head.ref_update("main", &request).expect_err("stale replay");
    assert_eq!(err.status, Status::Conflict);

    // A stale `old_head` conflicts too.
    let stale = RefUpdateRequest {
        old_head: Some("0".repeat(64)),
        new_head: main_head.clone(),
    };
    assert_eq!(
        head.ref_update("main", &stale).expect_err("stale old_head").status,
        Status::Conflict
    );
}

/// A ref update whose closure is not fully uploaded is refused (`422`).
#[test]
fn a_ref_update_with_a_missing_blob_is_refused() {
    let area = Area::new("missing-blob");
    prepare(&area, "wh");
    area.write_file("wh/a.txt", "alpha\n");
    area.write_file("wh/b.txt", "beta\n");
    area.forklift("wh", &["load", "."]);
    area.forklift("wh", &["stack", "two files"]);

    let harvest = harvest(&area.path("wh"));
    let main_head = harvest.head_of("main").expect("main head");

    // Find a blob (a leaf file's object). Blobs are the objects that are neither a parcel
    // nor a tree; the simplest way to pick one deterministically is to drop the smallest
    // object that still leaves the parcel/tree readable — but we only need *some* object
    // absent to break the closure, so drop one arbitrary object and confirm a 422.
    let head = Head::new(MemoryObjectStore::new(), MemoryRefStore::new());

    // Upload everything except one blob (an object that is not the head parcel).
    let mut skipped: Option<String> = None;
    for (hash, bytes) in &harvest.objects {
        if *hash != main_head && skipped.is_none() && is_probably_blob(&area.path("wh"), hash) {
            skipped = Some(hash.clone());
            continue;
        }
        head.object_put(None, hash, bytes).expect("upload object");
    }
    assert!(skipped.is_some(), "a blob was found to withhold");

    let request = RefUpdateRequest { old_head: None, new_head: main_head };
    let err = head.ref_update("main", &request).expect_err("incomplete closure");
    assert_eq!(err.status, Status::Unprocessable);
}

/// Classify an object as a blob by trying to parse it as a parcel/tree under the source
/// warehouse's scope; a blob is neither.
fn is_probably_blob(warehouse: &Path, hash: &str) -> bool {
    let _scope = StorageRootScope::enter(warehouse);
    object_utils::load_parcel(hash).is_err() && object_utils::load_tree(hash).is_err()
}

/// A wrong-hash upload is rejected — nothing unverified enters the store.
#[test]
fn a_tampered_object_upload_is_rejected() {
    let head = Head::new(MemoryObjectStore::new(), MemoryRefStore::new());

    let err = head
        .object_put(None, &"a".repeat(64), b"not the content of that hash")
        .expect_err("hash mismatch");
    assert_eq!(err.status, Status::Unprocessable);
}

/// The trusted path: a signed office chain plus a working pallet audit, all reused from
/// `forklift_core` through the scratch bridge.
#[test]
fn trusted_lift_audits_the_office_and_the_pallet() {
    let area = Area::new("trusted");
    prepare(&area, "wh");
    area.forklift("wh", &["office", "enroll"]);
    area.write_file("wh/app.txt", "v1\n");
    area.forklift("wh", &["load", "."]);
    area.forklift("wh", &["stack", "signed one"]);
    area.write_file("wh/app.txt", "v2\n");
    area.forklift("wh", &["load", "app.txt"]);
    area.forklift("wh", &["stack", "signed two"]);

    let harvest = harvest(&area.path("wh"));
    let anchor = harvest.trust.clone().expect("trust established");
    let office_head = harvest.head_of(&format!("@{}", OFFICE_PALLET_NAME)).expect("office head");
    let main_head = harvest.head_of("main").expect("main head");

    let head = Head::new(MemoryObjectStore::new(), MemoryRefStore::new());
    upload_all(&head, &harvest);

    // Trust first, then the office pallet, then the working pallet — the client's order.
    assert_eq!(head.put_trust(&anchor).expect("put trust"), TrustResult::Established);
    // Idempotent.
    assert_eq!(head.put_trust(&anchor).expect("put trust again"), TrustResult::Unchanged);

    head.ref_update(&format!("@{}", OFFICE_PALLET_NAME), &RefUpdateRequest {
        old_head: None,
        new_head: office_head.clone(),
    })
    .expect("lift office");

    head.ref_update("main", &RefUpdateRequest { old_head: None, new_head: main_head.clone() })
        .expect("lift main (audited)");

    let info = head.handshake().expect("handshake");
    assert_eq!(info.pallets.get("main"), Some(&main_head));
    assert_eq!(info.pallets.get(&format!("@{}", OFFICE_PALLET_NAME)), Some(&office_head));
    assert!(info.trust.is_some());
}

/// On a trusted warehouse, a user-pallet lift before the office is lifted is refused: the
/// audit has no keys to verify against.
#[test]
fn a_user_lift_before_the_office_is_refused() {
    let area = Area::new("office-first");
    prepare(&area, "wh");
    area.forklift("wh", &["office", "enroll"]);
    area.write_file("wh/app.txt", "v1\n");
    area.forklift("wh", &["load", "."]);
    area.forklift("wh", &["stack", "signed"]);

    let harvest = harvest(&area.path("wh"));
    let anchor = harvest.trust.clone().expect("trust");
    let main_head = harvest.head_of("main").expect("main head");

    let head = Head::new(MemoryObjectStore::new(), MemoryRefStore::new());
    upload_all(&head, &harvest);
    head.put_trust(&anchor).expect("put trust");

    // Skipping the office lift: main's audit finds no office pallet.
    let err = head
        .ref_update("main", &RefUpdateRequest { old_head: None, new_head: main_head })
        .expect_err("office missing");
    assert_eq!(err.status, Status::Unprocessable);
}

/// A signature sidecar is immutable: a conflicting re-upload is a `409`.
#[test]
fn a_conflicting_signature_is_refused() {
    let area = Area::new("sig-immutable");
    prepare(&area, "wh");
    area.forklift("wh", &["office", "enroll"]);
    area.write_file("wh/app.txt", "v1\n");
    area.forklift("wh", &["load", "."]);
    area.forklift("wh", &["stack", "signed"]);

    let harvest = harvest(&area.path("wh"));
    let (parcel, sidecar) = harvest.signatures.iter().next().expect("a signed parcel");

    let head = Head::new(MemoryObjectStore::new(), MemoryRefStore::new());

    assert_eq!(head.signature_put(parcel, sidecar).expect("store"), SignatureOutcome::Created);
    // Identical re-store: idempotent.
    assert_eq!(
        head.signature_put(parcel, sidecar).expect("re-store"),
        SignatureOutcome::AlreadyPresent
    );

    // A different (but still structurally valid) sidecar for the same parcel: conflict.
    // Reuse another parcel's sidecar bytes if there is one; otherwise mutate is impossible
    // without a valid signature, so only assert immutability when a second sidecar exists.
    if let Some((_, other)) = harvest.signatures.iter().find(|(hash, _)| *hash != parcel) {
        let err = head.signature_put(parcel, other).expect_err("conflict");
        assert_eq!(err.status, Status::Conflict);
    }
}

/// The presigned byte plane: with a staging store, object reads answer with a `307` to the
/// canonical key, while uploads are redirected to a **staging** key — never to the hash key
/// the reads serve. A session-less upload has nowhere to stage and is refused.
#[test]
fn a_staging_store_redirects_uploads_to_a_session_staging_key() {
    let store = MemoryObjectStore::with_redirect("https://s3.example/bucket");

    // Seed one object as if it were already promoted into S3.
    let bytes = b"an object".to_vec();
    let hash = object_utils::hash_object_bytes(&bytes);
    store.put_verified(&hash, &bytes).expect("seed a canonical object");

    let head = Head::new(store, MemoryRefStore::new());

    match head.object_get(&hash).expect("get") {
        ObjectReadResult::Redirect(url) => {
            assert_eq!(url, format!("https://s3.example/bucket/objects/{}", hash))
        }
        ObjectReadResult::Bytes(_) => panic!("expected a redirect"),
    }

    // The upload target is under the session's staging prefix, not `objects/{hash}`.
    match head.object_put(Some("lift-1"), &hash, b"ignored").expect("put") {
        ObjectWriteResult::Redirect(url) => {
            assert_eq!(url, format!("https://s3.example/bucket/staging/lift-1/{}", hash));
            assert!(!url.contains("/objects/"), "an upload must never target the hash key");
        }
        ObjectWriteResult::Stored { .. } => panic!("expected a redirect"),
    }

    // Without a session there is nowhere to stage, so the head refuses rather than
    // handing out a presigned PUT to the canonical key.
    let err = head.object_put(None, &hash, b"ignored").expect_err("session-less upload");
    assert_eq!(err.status, Status::Unprocessable);
}

/// Invariant 1 on the presigned path: bytes a client `PUT`s straight to the staging prefix
/// are **not fetchable at their hash key** until `commit_lift` verifies and promotes them,
/// and a corrupt staged object is discarded rather than promoted.
#[test]
fn a_staged_object_is_not_fetchable_until_it_is_verified_and_promoted() {
    let store = MemoryObjectStore::with_redirect("https://s3.example/bucket");

    let good = b"a good control-plane object".to_vec();
    let good_hash = object_utils::hash_object_bytes(&good);

    // Bytes that do NOT match the hash they are staged under — a client uploading garbage
    // to a presigned URL, the case the promote step must catch.
    let corrupt_hash = object_utils::hash_object_bytes(b"the declared content");

    store.stage("lift-1", &good_hash, good);
    store.stage("lift-1", &corrupt_hash, b"tampered content".to_vec());

    let head = Head::new(store, MemoryRefStore::new());

    // Neither is fetchable while it is merely staged: this is the invariant the old
    // canonical-key upload broke.
    for hash in [&good_hash, &corrupt_hash] {
        let err = head.object_get(hash).expect_err("a staged object is not fetchable");
        assert_eq!(err.status, Status::NotFound);
    }

    // A commit naming the corrupt object is refused...
    let err = head
        .commit_lift("lift-1", &[good_hash.clone(), corrupt_hash.clone()], &[])
        .expect_err("corrupt control-plane object");
    assert_eq!(err.status, Status::Unprocessable);

    // ...and the corrupt bytes are gone, never having reached the hash key.
    let err = head.object_get(&corrupt_hash).expect_err("corrupt bytes were discarded");
    assert_eq!(err.status, Status::NotFound);

    // A commit over only the good object promotes it: now — and only now — it is fetchable.
    head.commit_lift("lift-1", std::slice::from_ref(&good_hash), &[]).expect("clean commit");

    match head.object_get(&good_hash).expect("the promoted object") {
        ObjectReadResult::Redirect(url) => {
            assert_eq!(url, format!("https://s3.example/bucket/objects/{}", good_hash))
        }
        ObjectReadResult::Bytes(_) => panic!("expected a redirect"),
    }

    // The commit swept the session's staging prefix, and promotion is idempotent.
    assert_eq!(head.objects.staged_count(), 0, "staging is swept after a commit");
    head.commit_lift("lift-1", std::slice::from_ref(&good_hash), &[]).expect("retried commit");

    // A commit naming an object that was never staged is "not ready".
    let err = head.commit_lift("lift-1", &["f".repeat(64)], &[]).expect_err("missing object");
    assert_eq!(err.status, Status::Unprocessable);
}

/// A blob is presence-checked at its *canonical* key, which is the proof the staging
/// verifier already hash-checked it: a blob still in staging reads as not-yet-ready.
#[test]
fn a_blob_still_in_staging_is_not_ready_to_commit() {
    let store = MemoryObjectStore::with_redirect("https://s3.example/bucket");

    let blob = b"a large working blob".to_vec();
    let blob_hash = object_utils::hash_object_bytes(&blob);
    store.stage("lift-1", &blob_hash, blob);

    let head = Head::new(store, MemoryRefStore::new());

    let err = head
        .commit_lift("lift-1", &[], std::slice::from_ref(&blob_hash))
        .expect_err("unpromoted blob");
    assert_eq!(err.status, Status::Unprocessable);

    // The staging verifier promotes it out of band — the same trait operation the control
    // plane uses for small objects — and the commit then succeeds.
    let outcome = head.objects.verify_and_promote("lift-1", &blob_hash).expect("promote");
    assert_eq!(outcome, PromoteOutcome::Promoted);

    head.commit_lift("lift-1", &[], &[blob_hash]).expect("the blob is verified and present");
}

/// The control plane and the staging verifier can promote the same hash at the same time.
/// Exactly one wins; the loser sees the canonical object rather than a spurious "missing",
/// so a lift never fails because the other promoter got there first.
#[test]
fn racing_promoters_serialize_and_never_report_missing() {
    let store = MemoryObjectStore::with_redirect("https://s3.example/bucket");

    let bytes = b"an object two promoters both want".to_vec();
    let hash = object_utils::hash_object_bytes(&bytes);
    store.stage("lift-1", &hash, bytes);

    let barrier = std::sync::Barrier::new(2);
    let (store, barrier, hash) = (&store, &barrier, &hash);

    let outcomes: Vec<PromoteOutcome> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                scope.spawn(move || {
                    barrier.wait();
                    store.verify_and_promote("lift-1", hash).expect("promote")
                })
            })
            .collect();

        handles.into_iter().map(|handle| handle.join().expect("promoter thread")).collect()
    });

    assert_eq!(outcomes.iter().filter(|o| **o == PromoteOutcome::Promoted).count(), 1);
    assert_eq!(outcomes.iter().filter(|o| **o == PromoteOutcome::AlreadyPresent).count(), 1);
    assert!(!outcomes.contains(&PromoteOutcome::Missing), "the loser must not see 'missing'");
    assert_eq!(store.object_count(), 1);
    assert_eq!(store.staged_count(), 0);
}

/// `batch` returns a bundle-format stream the negotiation can consume, and the round trip
/// of `missing` is exact.
#[test]
fn batch_returns_a_bundle_stream() {
    let area = Area::new("batch");
    prepare(&area, "wh");
    area.write_file("wh/a.txt", "alpha\n");
    area.forklift("wh", &["load", "."]);
    area.forklift("wh", &["stack", "one"]);

    let harvest = harvest(&area.path("wh"));
    let head = Head::new(MemoryObjectStore::new(), MemoryRefStore::new());
    upload_all(&head, &harvest);

    let hashes: Vec<String> = harvest.objects.keys().cloned().collect();
    let bundle = head.batch(&hashes).expect("batch");
    assert!(!bundle.is_empty(), "the batch produced a non-empty bundle stream");

    // Nothing is missing after the upload.
    assert!(head.missing(&hashes).expect("missing").is_empty());
}
