/// The type of difference between two lines.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum DiffType {
    /// Added lines.
    Add,
    /// No operation. The line is the same in both files. This is not really a difference,
    /// it should only be used when the full file has to be displayed (including unchanged lines).
    NoOp,
    /// Removed lines.
    Remove
}