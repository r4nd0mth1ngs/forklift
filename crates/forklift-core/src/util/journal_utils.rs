use std::collections::BTreeMap;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use crate::globals::bay_root;
use crate::util::merge_utils::{self, ConsolidationState};
use crate::util::pallet_utils::{self, PalletRef};
use crate::util::file_utils;

/// The undo journal (§7.8): an append-only list of pre-operation state snapshots kept in
/// the bay, so `undo` can reverse the last state-changing operation — not just the last
/// `stack`. Content addressing makes this cheap: a snapshot is just the pallet refs, the
/// current pallet and any in-progress consolidation (all tiny); the objects they point at
/// are immutable and already stored. The working directory and inventory are the undo
/// command's concern, not the journal's.
const FILE_NAME_JOURNAL: &str = "journal.json";

/// Cap the journal so it cannot grow without bound; the oldest entries fall off (undo is
/// a recent-history convenience, not an audit trail — that is what `history`/`audit` are).
const MAX_ENTRIES: usize = 100;

/// One journaled operation: the mutable warehouse state *before* it ran.
#[derive(Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    /// The command that produced this snapshot (`stack`, `consolidate`, `shift`, …).
    pub op: String,

    /// The current pallet before the operation.
    pub current_pallet: String,

    /// Every pallet ref (user and meta) before the operation, keyed by wire form
    /// (`main`, `@office`) → head hash.
    pub heads: BTreeMap<String, String>,

    /// Any consolidation in progress before the operation (`their_head`, `their_pallet`).
    pub consolidation: Option<(String, String)>,
}

impl JournalEntry {
    /// Whether two entries describe the same mutable state (ignoring which op produced
    /// them) — used to avoid journaling a no-op command.
    fn same_state(&self, other: &JournalEntry) -> bool {
        self.current_pallet == other.current_pallet
            && self.heads == other.heads
            && self.consolidation == other.consolidation
    }
}

/// Capture the current mutable state as a would-be journal entry for `op`.
///
/// # Returns
/// * `Ok(JournalEntry)` - The snapshot.
/// * `Err(String)`      - If the refs or consolidation state could not be read.
pub fn capture(op: &str) -> Result<JournalEntry, String> {
    let current_pallet = pallet_utils::get_current_pallet_name()?;

    let heads = pallet_utils::all_pallet_refs()?
        .into_iter()
        .map(|(reference, head)| (reference.to_wire(), head))
        .collect();

    let consolidation = merge_utils::read_consolidation_state()?
        .map(|state| (state.their_head, state.their_pallet));

    Ok(JournalEntry { op: op.to_string(), current_pallet, heads, consolidation })
}

/// Append `pre` to the journal, but only if the state actually changed since it was
/// captured (so a command that turned out to be a no-op leaves no undo entry). Best-effort
/// by contract: the caller ignores the result, because a journaling hiccup must never fail
/// a command that already succeeded.
///
/// # Arguments
/// * `pre` - The pre-operation snapshot captured by [`capture`].
pub fn push_if_changed(pre: JournalEntry) -> Result<(), String> {
    // The undo journal reverses an operation by restoring refs, which cannot un-create a
    // pallet's very first parcel (that would need deleting the ref). So a stack onto an
    // unborn pallet is not journaled — `undo` falls back to its classic "nothing to undo
    // at the first parcel" handling there.
    if !pre.heads.contains_key(&pre.current_pallet) {
        return Ok(());
    }

    let now = capture(&pre.op)?;

    if pre.same_state(&now) {
        return Ok(());
    }

    let mut entries = read();
    entries.push(pre);

    let len = entries.len();
    if len > MAX_ENTRIES {
        entries.drain(0..len - MAX_ENTRIES);
    }

    write(&entries)
}

/// Remove and return the most recent journal entry.
///
/// # Returns
/// * `Ok(Some(JournalEntry))` - The popped entry.
/// * `Ok(None)`               - If the journal is empty.
/// * `Err(String)`            - If the journal could not be rewritten.
pub fn pop() -> Result<Option<JournalEntry>, String> {
    let mut entries = read();
    let popped = entries.pop();

    if popped.is_some() {
        write(&entries)?;
    }

    Ok(popped)
}

/// Restore the pallet refs, current pallet and consolidation state from an entry. The
/// working directory and inventory are left untouched — the `undo` command decides whether
/// to re-materialize (a soft reset keeps them; reversing a `shift` re-materializes).
///
/// # Arguments
/// * `entry` - The snapshot to restore.
pub fn restore_refs(entry: &JournalEntry) -> Result<(), String> {
    for (wire, head) in &entry.heads {
        let reference = PalletRef::parse(wire)?;
        pallet_utils::set_pallet_head_in(reference.namespace, &reference.name, head)?;
    }

    pallet_utils::set_current_pallet_name(&entry.current_pallet)?;

    match &entry.consolidation {
        Some((their_head, their_pallet)) => merge_utils::write_consolidation_state(
            &ConsolidationState { their_head: their_head.clone(), their_pallet: their_pallet.clone() }
        )?,
        None => merge_utils::clear_consolidation_state()?,
    }

    Ok(())
}

/// The journal file path (bay-local).
fn path() -> PathBuf {
    bay_root().join(FILE_NAME_JOURNAL)
}

/// Read the journal, tolerating a missing or corrupt file by treating it as empty — a
/// broken journal must never block a command.
fn read() -> Vec<JournalEntry> {
    let path = path();

    if !path.exists() {
        return Vec::new();
    }

    std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

/// Write the journal atomically.
fn write(entries: &[JournalEntry]) -> Result<(), String> {
    let json = serde_json::to_string(entries)
        .map_err(|e| format!("Error while serializing the undo journal: {}", e))?;

    file_utils::write_file_atomically(&path(), json.as_bytes())
}
