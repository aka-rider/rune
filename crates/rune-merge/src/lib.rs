//! Terminal-free 3-way hunk engine for rune's merge mode.
//!
//! [`merge_hunks`] classifies a 3-way merge into ordered [`Hunk`]s. `diffy`
//! is used only to locate conflict boundaries; every byte returned in a
//! [`Hunk`] is re-anchored verbatim into the original `ours`/`theirs` inputs
//! (never diffy's reserialized output), preserving line endings, BOM, and
//! trailing-newline state exactly.

mod hunks;

pub use hunks::{Hunk, merge_hunks};
