//! Code folding ranges.

/// A foldable range, expressed as a half-open character range `[start, end)`.
///
/// The range covers the whole syntax node that can be folded (e.g. a block or
/// function body). The first line of the node is kept visible as the fold
/// "header", the remaining lines are hidden when the fold is collapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FoldRange {
    /// Character offset of the first character of the foldable node.
    pub start: usize,
    /// Character offset just past the last character of the foldable node.
    pub end: usize,
}

impl FoldRange {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}
