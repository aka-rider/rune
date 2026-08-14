//! Named invariant checkers over `Snapshot`/`StepCtx`, using a
//! `Violation` + `Trunc` shape with a per-domain checker style.
//!
//! Three checker shapes, all pure and all over owned data, so every one is
//! independently unit-testable (plan Risk R-c):
//! - L0: `fn(&Snapshot) -> Option<Violation>` — a single-state property.
//! - L1: `fn(&Snapshot, &Snapshot) -> Option<Violation>` — a transition.
//! - L2: `fn(&Snapshot, &Snapshot, &StepCtx) -> Option<Violation>` — a
//!   transition that also needs what message caused it.
//!
//! Two invariants (`SYNC-IDEMPOTENT`, `WRAP-RT`) need data `Snapshot`
//! structurally cannot hold even with `StepCtx` added: a SECOND
//! `sync_view()` call's rendered rows, and the live `WrapSnapshot` itself
//! (`ViewSnapshots` derives nothing, G16, so it can't be cached in
//! `Snapshot` either). Those two are driven directly against `&mut App` /
//! `&ViewSnapshots` in `driver.rs`, which builds their pure, hand-testable
//! comparator inputs and calls into `render`/`wrap` below. `UNDO-TOTAL`/
//! `REDO-TOTAL` run once, at session end, also orchestrated by
//! `driver.rs` (they need to actually press undo/redo repeatedly).
//! `NO-PANIC` is not a checker function anywhere here — the driver
//! constructs it directly from a caught unwind.
//!
//! 39 invariants total, one domain per file:
//! - `cursor` — `CUR-BOUNDS`, `CUR-ORDER`, `CUR-ID`, `CUR-NO-CARET-HIDDEN`
//! - `nav` — `NAV-BOUNDS`
//! - `buffer` — `BUF-LINE-INDEX`, `VERSION-MONOTONE`
//! - `pane` — `PANE-NO-BLEED`, `LAYOUT-FITS`, `LAYOUT-TILES`
//! - `render` — `SYNC-IDEMPOTENT`, `CELL-OFFSET`, `CELL-NO-EOL`,
//!   `CELL-ORDER`, `TABLE-ROW-WIDTH`, `TABLE-SYNTHETIC-DECORATIVE`
//! - `wrap` — `WRAP-RT`
//! - `undo` — `REDO-CLEAR`, `UNDO-TOTAL`, `REDO-TOTAL`
//! - `session` — `SAVE-INFLIGHT-SM`, `QUIT-CHORD`, `CONFIRM-GEN`,
//!   `GUARD-ANSWERED` (plan WP2)
//! - `save` — `SAVE-VERBATIM`, `SAVE-CLEAN-MATCHES-DISK`
//! - `clipboard` — `PASTE-VERBATIM`, `CLIP-OSC52`
//! - `highlight` — `HL-CLAMPED`, `HL-STALE-DROP`, `HL-NO-REFLOW` (plan WP7)
//! - `merge` — `MERGE-DOC-ACTIVE`, `MERGE-SAVE-BLOCKED`,
//!   `MERGE-KEY-FEEDBACK`, `MERGE-TITLE-CLEARED` (plan WP7.S1), and the
//!   stateful `MERGE-NO-INSTANT-REDIVERGENCE` tracker — like
//!   `SAVE-SINGLE-FLIGHT`, driven by `driver.rs` per step, not a
//!   `check_all` entry (it needs history a `(prev, next, ctx)` triple
//!   cannot carry)
//! - `SAVE-SINGLE-FLIGHT` — constructed directly by `driver.rs`, not a
//!   checker function here (like `NO-PANIC`): a second in-flight save
//!   `Cmd` arriving while one is already pending is itself the violation
//!   (CODE-REVIEW.md rune-fuzz finding 3).

mod buffer;
mod clipboard;
mod cursor;
mod highlight;
mod merge;
mod nav;
mod pane;
mod render;
mod save;
mod session;
mod undo;
mod wrap;

pub use buffer::{buf_line_index, version_monotone};
pub use clipboard::{clip_osc52, paste_verbatim};
pub use cursor::{cur_bounds, cur_id, cur_no_caret_hidden, cur_order};
pub use highlight::{hl_clamped, hl_no_reflow, hl_stale_drop};
pub use merge::{
    DivergentSaveTracker, RedivergenceTracker, merge_doc_active, merge_key_feedback,
    merge_save_blocked, merge_title_cleared,
};
pub use nav::nav_bounds;
pub use pane::{layout_fits, layout_tiles, pane_no_bleed};
pub use render::{
    cell_no_eol, cell_offset, cell_order, sync_idempotent, sync_idempotent_rebuild,
    table_row_width, table_synthetic_decorative,
};
pub use save::{save_clean_matches_disk, save_verbatim};
pub use session::{confirm_gen, guard_answered, quit_chord, save_inflight_sm};
pub use undo::{redo_clear, redo_total, undo_total};
pub use wrap::{wrap_line_lens, wrap_rt};

use std::fmt;

use crate::guard::PanicSite;
use crate::snapshot::Snapshot;
use crate::step::StepCtx;

/// A failed invariant check. `site` is present only on a `NO-PANIC`
/// violation, whose producer the message alone never names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub id: &'static str,
    pub message: String,
    pub site: Option<PanicSite>,
}

impl Violation {
    pub fn new(id: &'static str, message: String) -> Violation {
        Violation {
            id,
            message,
            site: None,
        }
    }

    pub fn panicked(message: String, site: Option<PanicSite>) -> Violation {
        Violation {
            id: "NO-PANIC",
            message,
            site,
        }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.id, self.message)?;
        self.site.as_ref().map_or(Ok(()), |site| {
            write!(f, "\n  panicked at {}", site.location)
        })
    }
}

/// Truncating formatter for message payloads. Never slices mid-character
/// (`clippy::indexing_slicing` is denied under `-D warnings`; `str::get`
/// returns `None` instead of panicking on a bad range).
pub fn trunc(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let end = s.floor_char_boundary(n);
    s.get(..end)
        .map_or_else(|| s.to_string(), |head| format!("{head}…"))
}

/// Runs every per-step checker this crate can express purely over
/// `(prev, next, ctx)`, first-wins, in the order below. `SYNC-IDEMPOTENT`
/// and `WRAP-RT` are deliberately NOT here — they need live `App`/
/// `ViewSnapshots` access and are checked separately by `driver.rs` on
/// sampled steps only (G19); `UNDO-TOTAL`/`REDO-TOTAL` run once, at
/// session end, also from `driver.rs`. `NO-PANIC` is constructed directly
/// from a caught unwind, never through here.
pub fn check_all(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    cur_bounds(next)
        .or_else(|| cur_order(next))
        .or_else(|| cur_id(next))
        .or_else(|| cur_no_caret_hidden(next))
        .or_else(|| buf_line_index(next))
        .or_else(|| version_monotone(prev, next))
        .or_else(|| cell_offset(next))
        .or_else(|| cell_no_eol(next))
        .or_else(|| cell_order(next))
        .or_else(|| table_row_width(next))
        .or_else(|| table_synthetic_decorative(next))
        .or_else(|| redo_clear(prev, next))
        .or_else(|| pane_no_bleed(prev, next, ctx))
        .or_else(|| layout_fits(next))
        .or_else(|| layout_tiles(next))
        .or_else(|| save_inflight_sm(prev, next, ctx))
        .or_else(|| quit_chord(prev, next, ctx))
        .or_else(|| confirm_gen(prev, next, ctx))
        .or_else(|| guard_answered(prev, next, ctx))
        .or_else(|| paste_verbatim(prev, next, ctx))
        .or_else(|| save_verbatim(ctx))
        .or_else(|| save_clean_matches_disk(next, ctx))
        .or_else(|| clip_osc52(prev, ctx))
        .or_else(|| hl_clamped(next))
        .or_else(|| hl_stale_drop(prev, next, ctx))
        .or_else(|| hl_no_reflow(prev, next, ctx))
        .or_else(|| merge_doc_active(next))
        .or_else(|| merge_save_blocked(prev, next, ctx))
        .or_else(|| merge_key_feedback(prev, next, ctx))
        .or_else(|| merge_title_cleared(next))
        .or_else(|| nav_bounds(next))
}
