//! Garbage collection of unreferenced objects (DESIGN.html §4.5).
//!
//! A failed or abandoned lift leaves verified objects with no ref pointing at them.
//! The collector marks everything reachable from the GC roots — every pallet head,
//! plus the parked parcels and an in-progress consolidation, when those local states
//! exist — and sweeps the rest, with an mtime grace period protecting the objects of
//! in-flight lifts.

use std::collections::{HashSet, VecDeque};
use std::time::SystemTime;
use crate::util::{audit_utils, file_utils, merge_utils, object_utils, pallet_utils, park_utils};

/// What a collection did.
pub struct GcStats {
    /// Objects examined.
    pub scanned: usize,

    /// Unreferenced objects deleted (their signature sidecars ride along).
    pub deleted: usize,

    /// Unreferenced objects kept because they are younger than the grace period
    /// (an in-flight lift may still be uploading their reachers).
    pub kept_recent: usize,
}

/// Collect the garbage of the active warehouse: delete every object no GC root
/// reaches, unless it was modified within the last `grace_seconds`.
///
/// # Arguments
/// * `grace_seconds` - The grace period; unreferenced objects younger than this stay.
///
/// # Returns
/// * `Ok(GcStats)` - What happened.
/// * `Err(String)` - If the live set could not be computed (nothing is deleted then)
///                   or a deletion failed.
pub fn collect_garbage(grace_seconds: u64) -> Result<GcStats, String> {
    let live = collect_live_set()?;

    let objects_root = std::path::PathBuf::from(file_utils::get_path_objects_root());
    let now = SystemTime::now();

    let mut stats = GcStats { scanned: 0, deleted: 0, kept_recent: 0 };

    let folders = std::fs::read_dir(&objects_root)
        .map_err(|e| format!("Error while reading the objects folder: {}", e))?;

    for folder in folders {
        let folder = folder.map_err(|e| format!("Error while listing the objects folder: {}", e))?;

        if !folder.path().is_dir() {
            continue;
        }

        let prefix = folder.file_name().to_string_lossy().to_string();

        // The pack folder holds packed objects, not loose ones; it is not a hash fan-out
        // folder, so skip it — its `.pack`/`.idx` files are not garbage. (Collecting inside
        // packs is a repack concern, not this loose sweep.)
        if prefix.len() != file_utils::OBJECT_HASH_FOLDER_PATH_CHARACTERS
            || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }

        let files = std::fs::read_dir(folder.path())
            .map_err(|e| format!("Error while reading an objects folder: {}", e))?;

        for file in files {
            let file = file.map_err(|e| format!("Error while listing an objects folder: {}", e))?;
            let name = file.file_name().to_string_lossy().to_string();

            // Sidecars are swept with their object, never on their own.
            if name.ends_with(".sig") {
                continue;
            }

            stats.scanned += 1;

            let hash = format!("{}{}", prefix, name);

            if live.contains(&hash) {
                continue;
            }

            let age_is_protected = file.metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|modified| now.duration_since(modified).ok())
                .map(|age| age.as_secs() < grace_seconds)
                // An unreadable mtime protects the object: never delete on doubt.
                .unwrap_or(true);

            if age_is_protected {
                stats.kept_recent += 1;
                continue;
            }

            std::fs::remove_file(file.path())
                .map_err(|e| format!("Error while deleting object {}: {}", hash, e))?;

            let sidecar = file.path().with_file_name(format!("{}.sig", name));

            if sidecar.exists() {
                std::fs::remove_file(&sidecar)
                    .map_err(|e| format!("Error while deleting the sidecar of {}: {}", hash, e))?;
            }

            stats.deleted += 1;
        }
    }

    Ok(stats)
}

/// Compute the live set: every parcel, tree and blob reachable from the GC roots.
/// Shared with `pack_utils::compact` (a repack keeps exactly the live set).
pub(crate) fn collect_live_set() -> Result<HashSet<String>, String> {
    let mut roots: Vec<String> = Vec::new();

    // Every pallet head across both namespaces — user *and* meta (the office chain is a
    // GC root, or its keys would be collected as unreachable).
    for (_, head) in pallet_utils::all_pallet_refs()? {
        roots.push(head);
    }

    roots.extend(park_utils::read_parked()?);

    if let Some(consolidation) = merge_utils::read_consolidation_state()? {
        roots.push(consolidation.their_head);
    }

    // A re-genesis anchor (§8.7) pins the replaced office chain as attested history;
    // the pin is a GC root, or the attested chain would be collected as unreachable.
    if let Some(anchor) = crate::util::office_utils::read_trust_anchor()? {
        if let Some(adopts) = anchor.adopts {
            roots.push(adopts);
        }
    }

    let parcels = audit_utils::collect_reachable_present(&roots)?;

    let mut live: HashSet<String> = HashSet::new();
    let mut tree_queue: VecDeque<String> = VecDeque::new();

    for parcel_hash in &parcels {
        live.insert(parcel_hash.clone());
        tree_queue.push_back(object_utils::load_parcel(parcel_hash)?.tree_hash);
    }

    while let Some(tree_hash) = tree_queue.pop_front() {
        if !live.insert(tree_hash.clone()) {
            continue;
        }

        let tree = object_utils::load_tree(&tree_hash)?;

        for (_, file) in tree.get_files() {
            live.insert(file.hash.clone());
        }

        for (_, subtree) in tree.get_subtrees() {
            tree_queue.push_back(subtree.hash.clone());
        }
    }

    Ok(live)
}
