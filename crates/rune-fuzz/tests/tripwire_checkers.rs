//! WP4.S3/S4 — one hand-built bad `Snapshot`/pair per WP3 checker, each
//! paired with a well-formed companion of the same shape that must NOT
//! fire (the Risk R-c pattern). Every checker is called DIRECTLY, not
//! through `invariant::check_all`, so first-wins ordering cannot mask a
//! case. Split out of the sibling `tripwire` test binary's session half
//! (500-line budget) — `Snapshot`'s fields are all `pub` (G16), so
//! a checker's input is built directly here with no need to drive a real
//! `App` through anything.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use ratatui::layout::Rect;
use rune_core::buffer::Buffer;
use rune_core::coords::DisplayRow;
use rune_core::cursor::{Cursor, CursorId};
use rune_fuzz::invariant::{buf_line_index, cur_bounds, cur_id, cur_order, version_monotone};
use rune_fuzz::snapshot::Snapshot;
use rune_tui::app::App;
use rune_tui::document::DocumentId;
use rune_tui::focus::FocusTarget;
use rune_tui::layout::{self, Geometry};
use rune_tui::pane::Pane;
use rune_vfs::{Mem, Vfs};

fn collapsed_cursor(id: u32, position: usize) -> Cursor {
    Cursor {
        position,
        anchor: position,
        desired_col: 0,
        id: CursorId::try_from(id).expect("test ids are non-zero"),
    }
}

fn selection_cursor(id: u32, anchor: usize, position: usize) -> Cursor {
    Cursor {
        position,
        anchor,
        desired_col: 0,
        id: CursorId::try_from(id).expect("test ids are non-zero"),
    }
}

/// A single line's `line_starts`/`line_ends`, computed the same way
/// `Buffer` does (`buffer.rs`'s `line_start`/`line_end`): starts at every
/// byte right after a `\n`, and the last line's end is `content.len()`.
fn line_bounds(content: &str) -> (Vec<usize>, Vec<usize>) {
    let mut starts = vec![0usize];
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    let mut ends = Vec::with_capacity(starts.len());
    for n in 0..starts.len() {
        if n + 1 < starts.len() {
            ends.push(starts[n + 1] - 1);
        } else {
            ends.push(content.len());
        }
    }
    (starts, ends)
}

/// `DocumentId`'s inner field is `pub(crate)` to `rune_tui` (G16), so the
/// only way a test outside that crate can obtain a value is through its
/// public API. `App::new` always mints its first (and here, only)
/// document as `DocumentId(NonZeroU64::MIN)`, so this is a deterministic
/// constant in practice — a fresh in-memory `App` exists only long enough
/// to read `.active` back out of it.
fn base_active_id() -> DocumentId {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    App::new(Buffer::new(""), None, vfs, None).active
}

/// A well-formed baseline `Geometry` at an ordinary 80x24 frame, the left
/// column hidden (a fresh `App`'s default) — `LAYOUT-FITS`'s well-formed
/// companion state. Each test that exercises it overrides the field it's
/// checking rather than hand-building a `Geometry` literal, since its
/// fields are not all independently constructible (plan WP7).
fn base_geometry() -> Geometry {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let app = App::new(Buffer::new(""), None, vfs, None);
    layout::geometry(Rect::new(0, 0, 80, 24), &app)
}

/// A well-formed baseline `Snapshot` over `content`: one valid cursor at
/// offset 0, a correctly derived line index, the editor focused with no
/// modal up, and otherwise-quiescent fields. Each test overrides exactly
/// the field(s) it's exercising.
fn base_snapshot(content: &str) -> Snapshot {
    let (line_starts, line_ends) = line_bounds(content);
    let line_count = line_starts.len();
    Snapshot {
        content: content.to_string(),
        version: 1,
        saved_version: 1,
        is_dirty: false,
        cursors: vec![collapsed_cursor(1, 0)],
        line_count,
        line_starts,
        line_ends,
        journal_pos: 0,
        journal_len: 0,
        journal_tip_strip_run: 0,
        save_in_flight: false,
        pending_quit: None,
        should_quit: false,
        status: String::new(),
        focus: Pane::Editor,
        focus_target: FocusTarget::Editor,
        modal_open: false,
        active: base_active_id(),
        title_text: String::new(),
        title_cursor: collapsed_cursor(1, 0),
        title_window: 0..0,
        filesearch_query: None,
        search_draft: None,
        palette_query: None,
        read_only: rune_tui::document::ReadOnly::No,
        caret_visible: true,
        reading_link_focus: None,
        cells: Vec::new(),
        row_meta: Vec::new(),
        highlight_spans: Vec::new(),
        highlight_version: 1,
        geometry: base_geometry(),
        guard: None,
        quit_intent_pending: None,
        dirty_by_doc: std::collections::BTreeMap::new(),
        save_in_flight_by_doc: std::collections::BTreeMap::new(),
        saved_version_by_doc: std::collections::BTreeMap::new(),
        merge_active: false,
        merge_pending: false,
        merge_doc: None,
        merge_unresolved: 0,
        scroll_row: DisplayRow(0),
        display_name_by_doc: std::collections::BTreeMap::new(),
        active_last_sync: None,
        message_posts: 0,
        nav_places: Vec::new(),
        nav_current: 0,
        buffer_len_by_doc: std::collections::BTreeMap::new(),
    }
}

// ---------------------------------------------------------------------
// WP4.S3 — negative detection tests, one per WP3 invariant.
// ---------------------------------------------------------------------

#[test]
fn cur_bounds_detects_past_the_end() {
    let mut snap = base_snapshot("abc");
    snap.cursors = vec![collapsed_cursor(1, snap.content.len() + 1)];
    let v = cur_bounds(&snap).expect("cursor past content.len() must trip CUR-BOUNDS");
    assert_eq!(v.id, "CUR-BOUNDS");
}

#[test]
fn cur_bounds_detects_mid_rune() {
    let mut snap = base_snapshot("é");
    snap.cursors = vec![collapsed_cursor(1, 1)]; // "é" is 2 bytes; 1 is mid-rune
    let v = cur_bounds(&snap).expect("a mid-rune cursor offset must trip CUR-BOUNDS");
    assert_eq!(v.id, "CUR-BOUNDS");
}

#[test]
fn cur_order_detects_overlap() {
    let mut snap = base_snapshot("abcdefgh");
    snap.cursors = vec![
        selection_cursor(1, 0, 5), // selection 0..5
        selection_cursor(2, 2, 2), // starts inside the first cursor's selection
    ];
    let v = cur_order(&snap).expect("overlapping cursor selections must trip CUR-ORDER");
    assert_eq!(v.id, "CUR-ORDER");
}

#[test]
fn cur_order_detects_two_coincident_collapsed_cursors() {
    // CODE-REVIEW.md rune-fuzz finding 6: two collapsed cursors sharing the
    // same position is the canonical multi-cursor defect (every edit
    // double-applies), but `cur_id` only checks id uniqueness -- distinct
    // ids at the same position used to pass every cursor invariant clean.
    let mut snap = base_snapshot("abcdefgh");
    snap.cursors = vec![collapsed_cursor(1, 3), collapsed_cursor(2, 3)];
    let v =
        cur_order(&snap).expect("two collapsed cursors at the same position must trip CUR-ORDER");
    assert_eq!(v.id, "CUR-ORDER");
}

#[test]
fn cur_id_detects_duplicate() {
    let mut snap = base_snapshot("abcdefgh");
    snap.cursors = vec![collapsed_cursor(1, 0), collapsed_cursor(1, 3)];
    let v = cur_id(&snap).expect("two cursors sharing an id must trip CUR-ID");
    assert_eq!(v.id, "CUR-ID");
}

#[test]
fn cur_id_detects_empty() {
    let mut snap = base_snapshot("abcdefgh");
    snap.cursors = vec![];
    let v = cur_id(&snap).expect("an empty cursor set must trip CUR-ID");
    assert_eq!(v.id, "CUR-ID");
}

#[test]
fn buf_line_index_detects_non_increasing_starts() {
    let mut snap = base_snapshot("a\nb");
    snap.line_starts = vec![0, 0]; // must be strictly increasing
    snap.line_ends = vec![1, 3];
    let v = buf_line_index(&snap)
        .expect("non-strictly-increasing line_starts must trip BUF-LINE-INDEX");
    assert_eq!(v.id, "BUF-LINE-INDEX");
}

#[test]
fn version_monotone_detects_regression() {
    let mut prev = base_snapshot("abc");
    prev.version = 5;
    prev.saved_version = 5;
    let mut next = base_snapshot("abc");
    next.version = 3; // regressed
    next.saved_version = 5;
    let v =
        version_monotone(&prev, &next).expect("a version regression must trip VERSION-MONOTONE");
    assert_eq!(v.id, "VERSION-MONOTONE");
}

// ---------------------------------------------------------------------
// WP4.S4 — paired false-positive companions: the same shape, well-formed,
// must NOT fire.
// ---------------------------------------------------------------------

#[test]
fn cur_bounds_accepts_position_at_content_len() {
    let mut snap = base_snapshot("abc");
    snap.cursors = vec![collapsed_cursor(1, snap.content.len())]; // == len is valid
    assert_eq!(cur_bounds(&snap), None);
}

#[test]
fn cur_bounds_accepts_char_boundaries() {
    let mut snap = base_snapshot("é");
    snap.cursors = vec![collapsed_cursor(1, 0), collapsed_cursor(2, 2)]; // both valid boundaries
    assert_eq!(cur_bounds(&snap), None);
}

#[test]
fn cur_order_accepts_touching_selections() {
    let mut snap = base_snapshot("abcdefgh");
    snap.cursors = vec![
        selection_cursor(1, 0, 3), // selection 0..3
        selection_cursor(2, 3, 6), // starts exactly where the first ends
    ];
    assert_eq!(cur_order(&snap), None);
}

#[test]
fn cur_id_accepts_distinct_ids() {
    let mut snap = base_snapshot("abcdefgh");
    snap.cursors = vec![collapsed_cursor(1, 0), collapsed_cursor(2, 3)];
    assert_eq!(cur_id(&snap), None);
}

#[test]
fn cur_id_accepts_nonzero_id() {
    let mut snap = base_snapshot("abcdefgh");
    snap.cursors = vec![collapsed_cursor(1, 0)];
    assert_eq!(cur_id(&snap), None);
}

#[test]
fn cur_id_accepts_nonempty() {
    // Distinct from `cur_id_accepts_nonzero_id` (single cursor): several
    // cursors, distinct non-zero ids, ascending non-overlapping positions —
    // exercises the "all ids distinct" scan over a multi-element set, not
    // just the single-cursor trivial case.
    let mut snap = base_snapshot("abcdefgh");
    snap.cursors = vec![
        collapsed_cursor(1, 0),
        collapsed_cursor(2, 3),
        collapsed_cursor(3, 6),
    ];
    assert_eq!(cur_id(&snap), None);
}

#[test]
fn buf_line_index_detects_the_named_off_by_one() {
    // CODE-REVIEW.md rune-fuzz finding 2: a monotone-only check let
    // line_starts=[0,1,2] (wrong) pass clean for "a\nbb\nccc", whose real
    // starts are [0,2,5] -- monotone (0<1<2) but not what the content says.
    let mut snap = base_snapshot("a\nbb\nccc");
    snap.line_starts = vec![0, 1, 2];
    snap.line_ends = vec![0, 1, 2];
    let v =
        buf_line_index(&snap).expect("the exact off-by-one line_starts must trip BUF-LINE-INDEX");
    assert_eq!(v.id, "BUF-LINE-INDEX");
}

#[test]
fn buf_line_index_accepts_well_formed_index() {
    let snap = base_snapshot("a\nb"); // line_bounds already derives a valid index
    assert_eq!(buf_line_index(&snap), None);
}

#[test]
fn version_monotone_accepts_monotone_progress() {
    let mut prev = base_snapshot("abc");
    prev.version = 5;
    prev.saved_version = 5;
    let mut next = base_snapshot("abc");
    next.version = 6;
    next.saved_version = 6;
    assert_eq!(version_monotone(&prev, &next), None);
}
