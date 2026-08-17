//! Terminal-free hunk engine for rune's merge mode.
//!
//! [`merge_hunks`] classifies a 3-way merge into ordered [`Hunk`]s when a
//! common ancestor is known. `diffy` is used only to locate conflict
//! boundaries; every byte returned in a [`Hunk`] is re-anchored verbatim
//! into the original `ours`/`theirs` inputs (never diffy's reserialized
//! output), preserving line endings, BOM, and trailing-newline state
//! exactly. [`merge_hunks_no_ancestor`] handles the case where no ancestor
//! is known, via a direct line diff between `ours` and `theirs`.

mod align;
mod hunks;

pub use align::{
    AlignmentMap, IntralineSpans, LineSpans, Region, RegionKind, align, intraline, line_starts,
};
pub use hunks::{Hunk, merge_hunks, merge_hunks_no_ancestor};
