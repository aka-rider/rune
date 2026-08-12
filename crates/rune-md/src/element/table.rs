//! GFM table element machine (plan WP1). Replaces the raw `Verbatim`
//! passthrough with a real parsed shape — alignments, per-row/per-cell
//! structure, cell inlines — while `emit` still renders it byte-identically
//! to the old passthrough (WP2 gives it Grid/Wrapped/Pivoted layout).
//!
//! No `width` field: width is a parameter threaded through `emit` from the
//! document root's own wrap state, never a value an element caches a copy
//! of (the repo rule: "a value has exactly one writer").

use crate::element::inline::Inline;
use rune_syntax::element::{ByteRange, InheritCtx, RevealSm, RevealState};

/// A cell's column alignment, from the `|:---|:---:|---:|` delimiter row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableAlign {
    None,
    Left,
    Center,
    Right,
}

/// One table cell. `range` is comrak's own cell sourcepos — for a
/// GFM-autocompleted cell padding a short row, `range.start == range.end`
/// (comrak pads/truncates every row to the table's column count).
#[derive(Clone, Debug)]
pub struct TableCellM {
    pub range: ByteRange,
    pub inlines: Vec<Inline>,
}

/// How a raw row's cell count compares to the table's own column count
/// (`aligns.len()`), read off comrak's own cell ranges rather than a pipe
/// count (`TableRowShape`'s producer, `parse::table::build_table`, spells
/// out why). The emitter matches this exhaustively, so a future variant is
/// a compile error at every call site instead of a silently narrower table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableRowShape {
    Exact,
    Padded,
    Truncated,
}

/// One table row — the header row or a body row. The `|---|---|` delimiter
/// row itself has no comrak node and is not modeled as a `TableRowM`; see
/// `TableM::sep_line`.
#[derive(Clone, Debug)]
pub struct TableRowM {
    pub line: usize,
    pub is_header: bool,
    pub shape: TableRowShape,
    pub cells: Vec<TableCellM>,
}

/// A GFM table. Decide policy: `cursors.any_in_lines(first_line, last_line)`
/// — the whole block reveals as a unit, mirroring `CodeFenceM`.
#[derive(Clone, Debug)]
pub struct TableM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub aligns: Vec<TableAlign>,
    pub rows: Vec<TableRowM>,
    /// The buffer line the (comrak-absent) `|---|---|` delimiter row sits
    /// on: `header_row.line + 1`, clamped to `last_line`.
    pub sep_line: usize,
    pub first_line: usize,
    pub last_line: usize,
    /// One `ByteRange` per physical line `range` spans, container-prefix
    /// aware — the same shape `VerbatimM::content_lines` used for the raw
    /// passthrough this replaces (`parse::per_line_content`'s docs). The
    /// Revealed emit path iterates this, never `range` whole, for the same
    /// reason every other multi-line block here does.
    pub content_lines: Vec<ByteRange>,
}

impl TableM {
    pub(crate) fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let (first, last) = (self.first_line, self.last_line);
        let want = ctx.grant.resolve(|| ctx.cursors.any_in_lines(first, last));
        let mut dirty = self.sm.transition(want);
        let child_ctx = ctx.child(self.sm.state());
        for row in &mut self.rows {
            for cell in &mut row.cells {
                for inline in &mut cell.inlines {
                    dirty |= inline.sync(&child_ctx);
                }
            }
        }
        dirty
    }

    pub(crate) fn reveal_state(&self) -> RevealState {
        self.sm.state()
    }
}
