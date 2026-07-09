use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use crate::globals::bay_root;
use crate::util::{fanout_utils, file_utils, graph_utils, lcs, object_utils};

/// The name of the consolidation-state file (inside the forklift root folder). While a
/// consolidation is in progress, it holds the head parcel hash of the pallet being
/// consolidated in (line 1) and that pallet's name (line 2). The next `stack` records the
/// hash as a second parent and removes the file.
const FILE_NAME_CONSOLIDATION: &str = "consolidation";

/// The maximum number of lines (per side) the line-level merge attempts. Larger files are
/// not merged line by line; they fall back to a whole-file conflict.
const MAX_MERGE_LINES: usize = 20_000;

/// The result of a three-way content merge.
pub struct MergeResult {
    /// The merged content (with conflict markers if `has_conflicts` is set).
    pub content: Vec<u8>,

    /// Whether the merge produced conflicts.
    pub has_conflicts: bool,
}

/// A consolidation in progress.
pub struct ConsolidationState {
    /// The head parcel hash of the pallet being consolidated in (the second parent of the
    /// upcoming merge parcel).
    pub their_head: String,

    /// The name of the pallet being consolidated in (informational).
    pub their_pallet: String,
}

/// Get the path of the consolidation-state file (bay-local: a merge in progress belongs
/// to the bay resolving it).
fn get_consolidation_state_path() -> PathBuf {
    bay_root().join(FILE_NAME_CONSOLIDATION)
}

/// Read the consolidation state, if a consolidation is in progress.
///
/// # Returns
/// * `Ok(Some(ConsolidationState))` - The state of the consolidation in progress.
/// * `Ok(None)`                     - If no consolidation is in progress.
/// * `Err(String)`                  - If the state file exists but is malformed.
pub fn read_consolidation_state() -> Result<Option<ConsolidationState>, String> {
    let path = get_consolidation_state_path();

    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Error while reading \"{}\": {}", path.to_string_lossy(), e))?;

    let mut lines = content.lines();
    let their_head = lines.next().unwrap_or("").to_string();
    let their_pallet = lines.next().unwrap_or("").to_string();

    let is_valid_hash = their_head.len() == 64 && their_head.bytes().all(|b| b.is_ascii_hexdigit());

    if !is_valid_hash || their_pallet.is_empty() {
        return Err(format!(
            "The consolidation state file \"{}\" is malformed; remove it to abort the \
            consolidation.",
            path.to_string_lossy()
        ));
    }

    Ok(Some(ConsolidationState { their_head, their_pallet }))
}

/// Write the consolidation state (atomically).
///
/// # Arguments
/// * `state` - The state to write.
///
/// # Returns
/// * `Ok(())`      - If the state was written.
/// * `Err(String)` - If the file could not be written.
pub fn write_consolidation_state(state: &ConsolidationState) -> Result<(), String> {
    file_utils::write_file_atomically(
        &get_consolidation_state_path(),
        format!("{}\n{}\n", state.their_head, state.their_pallet).as_bytes()
    )
}

/// Remove the consolidation state file (a no-op when none exists).
///
/// # Returns
/// * `Ok(())`      - If the state file is gone.
/// * `Err(String)` - If the file exists but could not be removed.
pub fn clear_consolidation_state() -> Result<(), String> {
    let path = get_consolidation_state_path();

    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Error while removing \"{}\": {}", path.to_string_lossy(), e)),
    }
}

/// Check whether `ancestor` is an ancestor of (or equal to) `descendant` in the parcel graph.
///
/// The fast path reads the commit-graph ([`graph_utils`]), not parcel objects, and prunes on
/// generation numbers: an ancestor never has a higher generation than its descendant, and each
/// step toward the parents strictly lowers it, so any parcel already at or below the target's
/// generation (and not the target itself) cannot lead to it — its whole sub-history is skipped.
/// On a deep history this turns an O(history) walk into one bounded by the generation gap.
///
/// The graph is an accelerator, never a source of truth. Computing a generation number needs a
/// parcel's *complete* ancestry present locally, which is not always so mid-sync (a diverged
/// remote head is fetched before its every deep ancestor is). When the graph cannot be
/// completed the query falls back to a plain object walk from the descendant — which only ever
/// touches the descendant's own reachable history and short-circuits at `ancestor`, so it needs
/// exactly the objects the answer depends on and no more. A stored generation is only ever
/// written from a fully-loaded ancestry, so the fast path, when it succeeds, is always exact.
///
/// # Arguments
/// * `ancestor`   - The parcel hash to look for.
/// * `descendant` - The parcel hash whose history is walked.
///
/// # Returns
/// * `Ok(bool)`    - Whether `ancestor` is reachable from `descendant`.
/// * `Err(String)` - If the answer's own history could not be read.
pub fn is_ancestor(ancestor: &str, descendant: &str) -> Result<bool, String> {
    match is_ancestor_via_graph(ancestor, descendant) {
        Ok(answer) => Ok(answer),
        // The graph could not be completed (an ancestor object is not present locally yet); the
        // plain walk needs only the descendant's own reachable history, so it answers regardless.
        Err(_) => is_ancestor_via_walk(ancestor, descendant),
    }
}

/// The generation-number-pruned ancestry check (see [`is_ancestor`]). Errors if the graph
/// cannot be built for either parcel (a missing object in its ancestry).
fn is_ancestor_via_graph(ancestor: &str, descendant: &str) -> Result<bool, String> {
    if ancestor == descendant {
        return Ok(true);
    }

    let target = graph_utils::generation(ancestor)?;
    if target > graph_utils::generation(descendant)? {
        // The candidate ancestor is newer than the parcel whose history we would walk, so it
        // cannot possibly be behind it.
        return Ok(false);
    }

    let mut queue: VecDeque<String> = VecDeque::new();
    let mut visited: HashSet<String> = HashSet::new();
    queue.push_back(descendant.to_string());

    while let Some(hash) = queue.pop_front() {
        if hash == ancestor {
            return Ok(true);
        }
        if !visited.insert(hash.clone()) {
            continue;
        }

        let node = graph_utils::node(&hash)?;
        // Only a parcel strictly above the target's generation can still reach it; at or below
        // it (and not the target, handled above) its ancestors are all older than the target.
        if node.generation <= target {
            continue;
        }
        for parent in node.parents {
            queue.push_back(parent);
        }
    }

    Ok(false)
}

/// The plain ancestry check: a breadth-first walk from `descendant` toward the roots, reading
/// parcel objects and short-circuiting at `ancestor`. The graph-free fallback for
/// [`is_ancestor`] — it touches only the descendant's reachable history.
fn is_ancestor_via_walk(ancestor: &str, descendant: &str) -> Result<bool, String> {
    let mut queue: VecDeque<String> = VecDeque::new();
    let mut visited: HashSet<String> = HashSet::new();
    queue.push_back(descendant.to_string());

    while let Some(hash) = queue.pop_front() {
        if hash == ancestor {
            return Ok(true);
        }
        if !visited.insert(hash.clone()) {
            continue;
        }
        for parent in object_utils::load_parcel(&hash)?.parents {
            queue.push_back(parent);
        }
    }

    Ok(false)
}

/// Find a merge base of two parcels: a best (highest-generation) common ancestor.
///
/// The fast path paints the ancestors of `a` and `b`, exploring the highest generation first
/// via the commit-graph ([`graph_utils`]). Because a parcel's generation strictly exceeds every
/// one of its ancestors', by the time a parcel is popped every flag that can reach it already
/// has — so the first parcel popped carrying *both* flags is a highest-generation common
/// ancestor (a lowest common ancestor of the two). The generation ordering also lets the search
/// stop there instead of walking either parcel's history to the roots.
///
/// As with [`is_ancestor`], the graph is only an accelerator: if a generation cannot be
/// computed (an ancestor object is not present locally yet), the query falls back to a plain
/// object walk that finds the closest common ancestor from `b`'s side.
///
/// # Arguments
/// * `a` - The first parcel hash.
/// * `b` - The second parcel hash.
///
/// # Returns
/// * `Ok(Some(String))` - The hash of a common ancestor.
/// * `Ok(None)`         - If the two parcels share no history.
/// * `Err(String)`      - If the histories needed to answer could not be read.
pub fn find_merge_base(a: &str, b: &str) -> Result<Option<String>, String> {
    match find_merge_base_via_graph(a, b) {
        Ok(base) => Ok(base),
        Err(_) => find_merge_base_via_walk(a, b),
    }
}

/// The generation-ordered merge-base search (see [`find_merge_base`]). Errors if the graph
/// cannot be built for either parcel.
fn find_merge_base_via_graph(a: &str, b: &str) -> Result<Option<String>, String> {
    if a == b {
        return Ok(Some(a.to_string()));
    }

    const FLAG_A: u8 = 1;
    const FLAG_B: u8 = 2;
    const FLAG_BOTH: u8 = FLAG_A | FLAG_B;

    let mut flags: HashMap<String, u8> = HashMap::new();
    let mut heap: BinaryHeap<(u32, String)> = BinaryHeap::new();
    let mut descended: HashSet<String> = HashSet::new();

    flags.insert(a.to_string(), FLAG_A);
    heap.push((graph_utils::generation(a)?, a.to_string()));
    *flags.entry(b.to_string()).or_insert(0) |= FLAG_B;
    heap.push((graph_utils::generation(b)?, b.to_string()));

    while let Some((_, hash)) = heap.pop() {
        let flag = flags[&hash];
        if flag == FLAG_BOTH {
            return Ok(Some(hash));
        }
        // Descend each parcel once; its flags are already complete by the time it is popped.
        if !descended.insert(hash.clone()) {
            continue;
        }

        for parent in graph_utils::node(&hash)?.parents {
            let entry = flags.entry(parent.clone()).or_insert(0);
            let before = *entry;
            *entry |= flag;
            if *entry != before {
                heap.push((graph_utils::generation(&parent)?, parent));
            }
        }
    }

    Ok(None)
}

/// The plain merge-base search: collect `a`'s ancestors, then walk `b`'s toward the roots and
/// return the first parcel common to both. The graph-free fallback for [`find_merge_base`].
fn find_merge_base_via_walk(a: &str, b: &str) -> Result<Option<String>, String> {
    let mut ancestors_of_a: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    queue.push_back(a.to_string());
    while let Some(hash) = queue.pop_front() {
        if !ancestors_of_a.insert(hash.clone()) {
            continue;
        }
        for parent in object_utils::load_parcel(&hash)?.parents {
            queue.push_back(parent);
        }
    }

    let mut visited: HashSet<String> = HashSet::new();
    queue.push_back(b.to_string());
    while let Some(hash) = queue.pop_front() {
        if ancestors_of_a.contains(&hash) {
            return Ok(Some(hash));
        }
        if !visited.insert(hash.clone()) {
            continue;
        }
        for parent in object_utils::load_parcel(&hash)?.parents {
            queue.push_back(parent);
        }
    }

    Ok(None)
}

/// Merge two derived versions of a text file against their common base, line by line
/// (a diff3-style three-way merge). Chunks changed on only one side merge cleanly; chunks
/// changed differently on both sides become conflicts, marked in the output:
///
/// ```text
/// <<<<<<< <ours_label>
/// (our lines)
/// ||||||| base
/// (base lines)
/// =======
/// (their lines)
/// >>>>>>> <theirs_label>
/// ```
///
/// The comparison is exact (whitespace matters — this is a merge, not a display diff).
///
/// # Arguments
/// * `base`         - The common ancestor's content.
/// * `ours`         - Our version's content.
/// * `theirs`       - Their version's content.
/// * `ours_label`   - The label for our side in conflict markers.
/// * `theirs_label` - The label for their side in conflict markers.
///
/// # Returns
/// * `MergeResult` - The merged content and whether it contains conflicts.
pub fn merge_file_contents(base: &[u8],
                           ours: &[u8],
                           theirs: &[u8],
                           ours_label: &str,
                           theirs_label: &str) -> MergeResult {
    let base_lines = split_lines(base);
    let our_lines = split_lines(ours);
    let their_lines = split_lines(theirs);

    let too_large = base_lines.len() > MAX_MERGE_LINES
        || our_lines.len() > MAX_MERGE_LINES
        || their_lines.len() > MAX_MERGE_LINES;

    if too_large {
        // Fall back to a whole-file conflict: correctness over cleverness.
        return conflict_chunk(&our_lines, &base_lines, &their_lines, ours_label, theirs_label);
    }

    // For each base line: the index of the matching line in ours / theirs (from the
    // longest common subsequence), or None where the side diverges.
    let matches_ours = lcs::lcs_matches(&base_lines, &our_lines);
    let matches_theirs = lcs::lcs_matches(&base_lines, &their_lines);

    let mut content: Vec<u8> = Vec::new();
    let mut has_conflicts = false;

    let mut b = 0usize;
    let mut o = 0usize;
    let mut t = 0usize;

    while b < base_lines.len() || o < our_lines.len() || t < their_lines.len() {
        // A stable point: the current base line is matched by both sides at exactly the
        // current cursors.
        let is_stable = b < base_lines.len()
            && matches_ours[b] == Some(o)
            && matches_theirs[b] == Some(t);

        if is_stable {
            content.extend_from_slice(base_lines[b]);
            b += 1;
            o += 1;
            t += 1;
            continue;
        }

        // An unstable chunk: scan forward to the next base line matched by both sides.
        let mut next_b = b;
        let (next_o, next_t) = loop {
            match (next_b < base_lines.len()).then(|| (matches_ours[next_b], matches_theirs[next_b])) {
                Some((Some(no), Some(nt))) => break (no, nt),
                Some(_) => next_b += 1,
                None => break (our_lines.len(), their_lines.len()),
            }
        };

        let base_chunk = &base_lines[b..next_b];
        let our_chunk = &our_lines[o..next_o];
        let their_chunk = &their_lines[t..next_t];

        if our_chunk == base_chunk {
            extend_lines(&mut content, their_chunk);
        } else if their_chunk == base_chunk || our_chunk == their_chunk {
            extend_lines(&mut content, our_chunk);
        } else {
            let conflict = conflict_chunk(our_chunk, base_chunk, their_chunk, ours_label, theirs_label);
            content.extend(conflict.content);
            has_conflicts = true;
        }

        b = next_b;
        o = next_o;
        t = next_t;
    }

    MergeResult { content, has_conflicts }
}

/// Check whether content looks like text that can be merged line by line.
/// A NUL byte anywhere marks the content as binary.
///
/// # Arguments
/// * `content` - The content to check.
///
/// # Returns
/// * `true`  - If the content can be merged line by line.
/// * `false` - If the content should be treated as binary.
pub fn is_mergeable_text(content: &[u8]) -> bool {
    !content.contains(&0)
}

/// One action a consolidation has to perform to merge "theirs" into "ours".
pub enum MergeAction {
    /// Take their version of the file (ours is unchanged since the base).
    TakeTheirs {
        path: String,
        hash: String,
        item_type: crate::enums::dir_entry_type::DirEntryType,
        /// Whether the file does not exist in our tree (an untracked file at the path is
        /// then a collision).
        is_new: bool,
    },

    /// Remove the file (they removed it; ours is unchanged since the base).
    Delete { path: String },

    /// Both sides changed the file and the line merge succeeded: write the merged content.
    Merged {
        path: String,
        content: Vec<u8>,
        item_type: crate::enums::dir_entry_type::DirEntryType,
    },

    /// The file is in conflict. When `content` is set, it is written to the working
    /// directory (line-merge conflicts with markers, or their content for a
    /// delete/modify conflict); when it is `None`, our file is kept as it is.
    Conflict {
        path: String,
        content: Option<Vec<u8>>,
        /// The hash recorded on the (stale, conflict-state) inventory entry.
        entry_hash: String,
        item_type: crate::enums::dir_entry_type::DirEntryType,
    },
}

/// One entry of the merge walk before its heavy work is run. The walk is a cheap tree
/// traversal that decides on the spot every action it can; a file both sides changed needs
/// a three-way line merge — the expensive part — so it records a [`MergeJob`] to run later,
/// in parallel. Resolving the pending list preserves walk order, so the final actions are
/// exactly what a single-threaded merge produced.
enum PendingAction {
    /// An action already decided during the walk.
    Ready(MergeAction),

    /// A file both sides changed: load its three sides and line-merge them (deferred so the
    /// merges fan out across the cores).
    Merge(MergeJob),
}

/// A deferred three-way line merge of one file both sides changed (see [`PendingAction`]).
/// The (expensive) blob loads and line diff are all done when the job is resolved, so the
/// walk that records it stays cheap.
struct MergeJob {
    path: String,

    /// The base blob hash, or `None` if the file was added on both sides.
    base_hash: Option<String>,
    our_hash: String,
    their_hash: String,
    our_type: crate::enums::dir_entry_type::DirEntryType,
    their_type: crate::enums::dir_entry_type::DirEntryType,
}

/// Compute the actions that merge the `theirs` tree into the `ours` tree, given their
/// common `base` tree. Subtrees that are equal between ours and theirs — or unchanged on
/// their side — are skipped entirely.
///
/// The three-way line merges of files both sides changed (the expensive part) fan out
/// across the cores; the result is identical to a single-threaded merge, in the same order.
///
/// # Arguments
/// * `base_hash`    - The root tree hash of the merge base.
/// * `ours_hash`    - The root tree hash of our head.
/// * `theirs_hash`  - The root tree hash of their head.
/// * `ours_label`   - The conflict-marker label for our side (the current pallet name).
/// * `theirs_label` - The conflict-marker label for their side.
///
/// # Returns
/// * `Ok(Vec<MergeAction>)` - The actions to perform.
/// * `Err(String)`          - If an object could not be loaded.
pub fn compute_merge_actions(base_hash: &str,
                             ours_hash: &str,
                             theirs_hash: &str,
                             ours_label: &str,
                             theirs_label: &str) -> Result<Vec<MergeAction>, String> {
    let base_tree = object_utils::load_tree(base_hash)?;
    let ours_tree = object_utils::load_tree(ours_hash)?;
    let theirs_tree = object_utils::load_tree(theirs_hash)?;

    // Phase 1 — walk the three trees and decide every cheap action; a file both sides
    // changed is recorded as a deferred merge (no blob load, no line diff yet).
    let mut pending: Vec<PendingAction> = Vec::new();

    merge_directory(
        Some(&base_tree),
        Some(&ours_tree),
        Some(&theirs_tree),
        "",
        &mut pending
    )?;

    // Phase 2 — run the deferred three-way merges across the cores and reassemble the
    // actions in walk order.
    resolve_pending(pending, ours_label, theirs_label)
}

/// The number of deferred merges below which running them on the calling thread is cheaper
/// than the threads that would share them.
const PARALLEL_MERGE_THRESHOLD: usize = 8;

/// Resolve the pending merge walk into the final action list: cheap actions pass straight
/// through, and the deferred three-way merges — the expensive line diffs — run (fanned
/// across the cores once there are enough of them), their results reassembled in the
/// original walk order.
fn resolve_pending(pending: Vec<PendingAction>,
                   ours_label: &str,
                   theirs_label: &str) -> Result<Vec<MergeAction>, String> {
    let total = pending.len();

    let mut ready: Vec<(usize, MergeAction)> = Vec::new();
    let mut jobs: Vec<(usize, MergeJob)> = Vec::new();

    for (index, entry) in pending.into_iter().enumerate() {
        match entry {
            PendingAction::Ready(action) => ready.push((index, action)),
            PendingAction::Merge(job) => jobs.push((index, job)),
        }
    }

    let resolved: Vec<(usize, Result<MergeAction, String>)> =
        if jobs.len() < PARALLEL_MERGE_THRESHOLD {
            jobs.iter()
                .map(|(index, job)| (*index, resolve_merge_job(job, ours_label, theirs_label)))
                .collect()
        } else {
            resolve_merge_jobs_parallel(&jobs, ours_label, theirs_label)
        };

    // Slot every action back into its walk position (each index is filled exactly once).
    let mut slots: Vec<Option<MergeAction>> = std::iter::repeat_with(|| None).take(total).collect();

    for (index, action) in ready {
        slots[index] = Some(action);
    }
    for (index, action) in resolved {
        slots[index] = Some(action?);
    }

    Ok(slots.into_iter().map(|slot| slot.expect("every walk slot is filled exactly once")).collect())
}

/// Run the deferred three-way merges across the cores, returning `(walk index, result)` so
/// the caller can slot each back into walk order.
fn resolve_merge_jobs_parallel(jobs: &[(usize, MergeJob)],
                               ours_label: &str,
                               theirs_label: &str) -> Vec<(usize, Result<MergeAction, String>)> {
    // See `fanout_utils::fanout_map` for the fan-out idiom (chunking, worker count, and the
    // storage-scope re-entry every worker needs).
    fanout_utils::fanout_map(jobs, |(index, job)| {
        (*index, resolve_merge_job(job, ours_label, theirs_label))
    })
}

/// Load one deferred file's three sides and three-way merge them — the body of the parallel
/// phase. It reads blobs through the shared, thread-safe object caches and the line merge is
/// pure CPU, so it is safe to run on many threads at once.
fn resolve_merge_job(job: &MergeJob,
                     ours_label: &str,
                     theirs_label: &str) -> Result<MergeAction, String> {
    use crate::enums::dir_entry_type::DirEntryType;

    let base_content = match &job.base_hash {
        Some(hash) => object_utils::load_blob(hash)?.content,
        None => Vec::new(),
    };
    let our_content = object_utils::load_blob(&job.our_hash)?.content;
    let their_content = object_utils::load_blob(&job.their_hash)?.content;

    let all_text = is_mergeable_text(&base_content)
        && is_mergeable_text(&our_content)
        && is_mergeable_text(&their_content);

    if !all_text {
        // Binary contents are not line-mergeable; keep ours, in conflict.
        return Ok(MergeAction::Conflict {
            path: job.path.clone(),
            content: None,
            entry_hash: job.our_hash.clone(),
            item_type: job.our_type,
        });
    }

    // The executable bit wins from either side (a side that turned the file executable
    // keeps it executable).
    let merged_type = if job.our_type == DirEntryType::Executable
        || job.their_type == DirEntryType::Executable {
        DirEntryType::Executable
    } else {
        DirEntryType::Normal
    };

    let result = merge_file_contents(
        &base_content,
        &our_content,
        &their_content,
        ours_label,
        theirs_label
    );

    Ok(if result.has_conflicts {
        MergeAction::Conflict {
            path: job.path.clone(),
            content: Some(result.content),
            entry_hash: job.our_hash.clone(),
            item_type: merged_type,
        }
    } else {
        MergeAction::Merged {
            path: job.path.clone(),
            content: result.content,
            item_type: merged_type,
        }
    })
}

/// Merge one directory level of the three trees (recursively).
fn merge_directory(base: Option<&crate::model::tree_item::TreeItem>,
                   ours: Option<&crate::model::tree_item::TreeItem>,
                   theirs: Option<&crate::model::tree_item::TreeItem>,
                   key: &str,
                   pending: &mut Vec<PendingAction>) -> Result<(), String> {
    use std::collections::BTreeMap;
    use crate::enums::dir_entry_type::DirEntryType;
    use crate::model::tree_item::TreeItem;

    let collect_files = |tree: Option<&TreeItem>| -> BTreeMap<String, (String, DirEntryType)> {
        tree.map(|t| t.get_files()
                .map(|(name, item)| (name.clone(), (item.hash.clone(), item.item_type)))
                .collect())
            .unwrap_or_default()
    };

    let base_files = collect_files(base);
    let our_files = collect_files(ours);
    let their_files = collect_files(theirs);

    let mut names: std::collections::BTreeSet<&String> = std::collections::BTreeSet::new();
    names.extend(base_files.keys());
    names.extend(our_files.keys());
    names.extend(their_files.keys());

    for name in names {
        let b = base_files.get(name);
        let o = our_files.get(name);
        let t = their_files.get(name);

        let path = if key.is_empty() { name.clone() } else { format!("{}/{}", key, name) };

        // Nothing to do when both sides agree, or their side is unchanged since the base.
        if o == t || t == b {
            continue;
        }

        // Our side is unchanged since the base: take their side.
        if o == b {
            match t {
                Some((hash, item_type)) => pending.push(PendingAction::Ready(MergeAction::TakeTheirs {
                    path,
                    hash: hash.clone(),
                    item_type: *item_type,
                    is_new: o.is_none(),
                })),
                None => pending.push(PendingAction::Ready(MergeAction::Delete { path })),
            }
            continue;
        }

        // Both sides changed the file (relative to the base) in different ways.
        match (o, t) {
            // Delete/modify: they changed it, we deleted it. Their version is put back
            // in the working directory, in conflict.
            (None, Some((their_hash, their_type))) => {
                let their_blob = object_utils::load_blob(their_hash)?;

                pending.push(PendingAction::Ready(MergeAction::Conflict {
                    path,
                    content: Some(their_blob.content),
                    entry_hash: their_hash.clone(),
                    item_type: *their_type,
                }));
            }

            // Modify/delete: we changed it, they deleted it. Our file is kept, in conflict.
            (Some((our_hash, our_type)), None) => {
                pending.push(PendingAction::Ready(MergeAction::Conflict {
                    path,
                    content: None,
                    entry_hash: our_hash.clone(),
                    item_type: *our_type,
                }));
            }

            (Some((our_hash, our_type)), Some((their_hash, their_type))) => {
                let both_plain_files = (*our_type == DirEntryType::Normal || *our_type == DirEntryType::Executable)
                    && (*their_type == DirEntryType::Normal || *their_type == DirEntryType::Executable);

                if !both_plain_files {
                    // Type conflicts (symlink vs file, …) are not line-mergeable; keep ours.
                    pending.push(PendingAction::Ready(MergeAction::Conflict {
                        path,
                        content: None,
                        entry_hash: our_hash.clone(),
                        item_type: *our_type,
                    }));
                    continue;
                }

                // Both sides changed a plain file: defer the three-way line merge (the blob
                // loads and the diff happen when the job is resolved, in parallel).
                pending.push(PendingAction::Merge(MergeJob {
                    path,
                    base_hash: b.map(|(base_hash, _)| base_hash.clone()),
                    our_hash: our_hash.clone(),
                    their_hash: their_hash.clone(),
                    our_type: *our_type,
                    their_type: *their_type,
                }));
            }

            (None, None) => unreachable!("handled by the o == t case"),
        }
    }

    // Recurse into subtrees.
    let collect_subtrees = |tree: Option<&TreeItem>| -> BTreeMap<String, String> {
        tree.map(|t| t.get_subtrees()
                .map(|(name, item)| (name.clone(), item.hash.clone()))
                .collect())
            .unwrap_or_default()
    };

    let base_subtrees = collect_subtrees(base);
    let our_subtrees = collect_subtrees(ours);
    let their_subtrees = collect_subtrees(theirs);

    let mut subtree_names: std::collections::BTreeSet<&String> = std::collections::BTreeSet::new();
    subtree_names.extend(base_subtrees.keys());
    subtree_names.extend(our_subtrees.keys());
    subtree_names.extend(their_subtrees.keys());

    for name in subtree_names {
        let b = base_subtrees.get(name);
        let o = our_subtrees.get(name);
        let t = their_subtrees.get(name);

        // Identical on both sides, or unchanged on their side: nothing to merge below.
        if o == t || t == b {
            continue;
        }

        let child_key = if key.is_empty() { name.clone() } else { format!("{}/{}", key, name) };

        let load = |hash: Option<&String>| -> Result<Option<TreeItem>, String> {
            match hash {
                Some(hash) => object_utils::load_tree(hash).map(Some),
                None => Ok(None),
            }
        };

        let base_loaded = load(b)?;
        let ours_loaded = load(o)?;
        let theirs_loaded = load(t)?;

        merge_directory(
            base_loaded.as_ref(),
            ours_loaded.as_ref(),
            theirs_loaded.as_ref(),
            &child_key,
            pending
        )?;
    }

    Ok(())
}

/// Build a conflict chunk with markers.
fn conflict_chunk(ours: &[&[u8]],
                  base: &[&[u8]],
                  theirs: &[&[u8]],
                  ours_label: &str,
                  theirs_label: &str) -> MergeResult {
    let mut content: Vec<u8> = Vec::new();

    content.extend(format!("<<<<<<< {}\n", ours_label).as_bytes());
    extend_lines(&mut content, ours);
    content.extend(b"||||||| base\n");
    extend_lines(&mut content, base);
    content.extend(b"=======\n");
    extend_lines(&mut content, theirs);
    content.extend(format!(">>>>>>> {}\n", theirs_label).as_bytes());

    MergeResult { content, has_conflicts: true }
}

/// Append the given lines to the content.
fn extend_lines(content: &mut Vec<u8>, lines: &[&[u8]]) {
    for line in lines {
        content.extend_from_slice(line);
    }
}

/// Split content into lines, each including its trailing new line byte (the last line may
/// lack one). Exact bytes — no whitespace normalization.
fn split_lines(content: &[u8]) -> Vec<&[u8]> {
    let mut lines: Vec<&[u8]> = Vec::new();
    let mut start = 0usize;

    for (index, byte) in content.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(&content[start..=index]);
            start = index + 1;
        }
    }

    if start < content.len() {
        lines.push(&content[start..]);
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A micro-benchmark (not run by default) contrasting the generation-number fast path with
    /// the plain object walk on a deep shared history with a recent fork — the realistic shape
    /// of a merge base or a divergence check. Run with:
    ///
    /// ```text
    /// cargo test -p forklift-core merge_base_bench -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn merge_base_bench_generations_beat_the_plain_walk() {
        use crate::builder::object::loose_object_builder::LooseObjectBuilder;
        use crate::globals::StorageRootScope;
        use crate::model::parcel::Parcel;
        use std::time::Instant;

        let temp = std::env::temp_dir().join(format!("forklift-mergebase-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let _scope = StorageRootScope::enter(&temp);

        // Ancestry never reads the tree, so a single dummy tree hash is enough for every parcel.
        // A per-parcel description keeps otherwise-identical parcels (the two tips) distinct —
        // content addressing would collapse them to one hash otherwise.
        let dummy_tree = "0".repeat(64);
        let store = |parents: Vec<String>, tag: &str| -> String {
            let parcel = Parcel {
                tree_hash: dummy_tree.clone(),
                parents,
                actions: vec![],
                description: Some(tag.to_string()),
            };
            let mut object = LooseObjectBuilder::build_parcel(&parcel);
            let hash = object.hash.clone();
            object.store().unwrap();
            hash
        };

        // A deep shared trunk, then two distinct tips forking off its newest parcel.
        const DEPTH: usize = 50_000;
        let mut prev: Vec<String> = vec![];
        let mut fork_point = String::new();
        for i in 0..DEPTH {
            let hash = store(prev.clone(), &format!("trunk {i}"));
            fork_point = hash.clone();
            prev = vec![hash];
        }
        let tip_a = store(vec![fork_point.clone()], "tip A");
        let tip_b = store(vec![fork_point.clone()], "tip B");

        let built = graph_utils::build_from_heads(&[tip_a.clone(), tip_b.clone()]).unwrap();
        assert!(built >= DEPTH, "the graph must cover the trunk");

        // find_merge_base: the plain walk collects the whole trunk; the graph stops at the fork.
        let t = Instant::now();
        let walk_base = find_merge_base_via_walk(&tip_a, &tip_b).unwrap();
        let walk_base_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let graph_base = find_merge_base_via_graph(&tip_a, &tip_b).unwrap();
        let graph_base_ms = t.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(walk_base, Some(fork_point.clone()));
        assert_eq!(graph_base, Some(fork_point.clone()));

        // is_ancestor for a *divergence* check (the common consolidate/lift/haul case): tip_b is
        // not an ancestor of tip_a. The plain walk scans all of tip_a's trunk to say "no"; the
        // graph prunes on the first parcel (tip_a is not above tip_b's generation).
        let t = Instant::now();
        let walk_anc = is_ancestor_via_walk(&tip_b, &tip_a).unwrap();
        let walk_anc_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let graph_anc = is_ancestor_via_graph(&tip_b, &tip_a).unwrap();
        let graph_anc_ms = t.elapsed().as_secs_f64() * 1000.0;
        assert!(!walk_anc && !graph_anc, "the tips diverged");

        println!("\n=== ancestry on a {DEPTH}-deep shared history (recent fork) ===");
        println!("find_merge_base : walk {walk_base_ms:8.2} ms  graph {graph_base_ms:8.3} ms  ({:.0}x)", walk_base_ms / graph_base_ms.max(0.0001));
        println!("is_ancestor(div): walk {walk_anc_ms:8.2} ms  graph {graph_anc_ms:8.3} ms  ({:.0}x)", walk_anc_ms / graph_anc_ms.max(0.0001));

        std::fs::remove_dir_all(&temp).ok();
    }

    fn merge(base: &str, ours: &str, theirs: &str) -> (String, bool) {
        let result = merge_file_contents(
            base.as_bytes(),
            ours.as_bytes(),
            theirs.as_bytes(),
            "ours",
            "theirs"
        );

        (String::from_utf8(result.content).unwrap(), result.has_conflicts)
    }

    #[test]
    fn non_overlapping_changes_merge_cleanly() {
        let base = "one\ntwo\nthree\nfour\nfive\n";
        let ours = "ONE\ntwo\nthree\nfour\nfive\n";
        let theirs = "one\ntwo\nthree\nfour\nFIVE\n";

        let (merged, conflicts) = merge(base, ours, theirs);
        assert!(!conflicts);
        assert_eq!(merged, "ONE\ntwo\nthree\nfour\nFIVE\n");
    }

    #[test]
    fn identical_changes_merge_cleanly() {
        let base = "one\ntwo\n";
        let both = "one\nTWO\n";

        let (merged, conflicts) = merge(base, both, both);
        assert!(!conflicts);
        assert_eq!(merged, both);
    }

    #[test]
    fn additions_on_one_side_are_taken() {
        let base = "one\ntwo\n";
        let ours = "one\ntwo\nthree\n";
        let theirs = "zero\none\ntwo\n";

        let (merged, conflicts) = merge(base, ours, theirs);
        assert!(!conflicts);
        assert_eq!(merged, "zero\none\ntwo\nthree\n");
    }

    #[test]
    fn competing_changes_conflict_with_markers() {
        let base = "one\ntwo\nthree\n";
        let ours = "one\nOURS\nthree\n";
        let theirs = "one\nTHEIRS\nthree\n";

        let (merged, conflicts) = merge(base, ours, theirs);
        assert!(conflicts);
        assert!(merged.contains("<<<<<<< ours\nOURS\n"));
        assert!(merged.contains("||||||| base\ntwo\n"));
        assert!(merged.contains("=======\nTHEIRS\n"));
        assert!(merged.contains(">>>>>>> theirs"));
        assert!(merged.starts_with("one\n"));
        assert!(merged.ends_with("three\n"));
    }

    #[test]
    fn whitespace_changes_are_real_changes() {
        // The display diff forgives whitespace-only line changes; the merge must not.
        let base = "line\n";
        let ours = "line \n";
        let theirs = "line\n";

        let (merged, conflicts) = merge(base, ours, theirs);
        assert!(!conflicts);
        assert_eq!(merged, "line \n");
    }

    #[test]
    fn files_without_trailing_newline_survive() {
        let base = "one\ntwo";
        let ours = "one\ntwo";
        let theirs = "one\ntwo\nthree";

        let (merged, conflicts) = merge(base, ours, theirs);
        assert!(!conflicts);
        assert_eq!(merged, "one\ntwo\nthree");
    }

    #[test]
    fn binary_content_is_not_mergeable() {
        assert!(is_mergeable_text(b"plain text\n"));
        assert!(!is_mergeable_text(b"bin\0ary"));
    }
}
