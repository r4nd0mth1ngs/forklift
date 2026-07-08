use crate::enums::diff_type::DiffType;

/// A difference between two lines.
pub struct Diff {
    /// The line number in the new file (usually for additions).
    pub line_number_new: Option<u32>,
    /// The line number in the old file (usually for removals).
    pub line_number_old: Option<u32>,
    pub diff_type: DiffType,
    /// Whether this line belongs to a whitespace-only change (reindentation, blank-line
    /// edits, a missing final new line, ...). The change is still real — the content hash
    /// changes — but presentation layers may dim or filter such lines.
    pub is_whitespace_only: bool,
    pub line: Vec<u8>
}
