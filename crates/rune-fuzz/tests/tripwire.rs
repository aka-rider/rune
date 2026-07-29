//! WP4 tripwire: proves the WP3 invariant net has no hole, deterministically
//! and once. Unlike `tests/human_session.rs`, every test here is NOT
//! `#[ignore]`d, so `make test` runs the whole file on every build (plan
//! WP4.S1).
//!
//! Two halves:
//! - one hand-written ~35-action session that must trip NOTHING
//!   (`clean_session_trips_nothing`, WP4.S2), paired with a determinism
//!   check (`driver_is_deterministic`, WP4.S5) — proof the driver itself,
//!   and the whole checker set, agree with well-formed production
//!   behaviour;
//! - one hand-built bad `Snapshot`/pair per WP3 checker
//!   (`*_detects_*`, WP4.S3), each paired with a well-formed companion of
//!   the same shape that must NOT fire (`*_accepts_*`, WP4.S4) — the Risk
//!   R-c pattern from the Go fuzzer's own workspace and display
//!   invariant tests. Every
//!   checker is called DIRECTLY, not through `invariant::check_all`, so
//!   first-wins ordering cannot mask a case.
//!
//! `clean_session_trips_nothing`'s fixture is curated per plan Gotcha G1:
//! plain ASCII + CJK markdown, no lists or blockquotes, no tab, no `\r` —
//! this file is NOT `#[ignore]`d, so it runs under `cargo test --workspace`,
//! which links `rune-md` with `strict-invariants` on (known-open comrak
//! sourcepos panics, `crates/rune-md/TODO.md`, Status: open).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::Cursor;
use rune_fuzz::action::Action;
use rune_fuzz::driver;
use rune_fuzz::invariant::{buf_line_index, cur_bounds, cur_id, cur_order, version_monotone};
use rune_fuzz::snapshot::Snapshot;
use rune_tui::app::App;
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_vfs::{Mem, Vfs};

/// Seed content for the positive tripwire session. Headers and paragraphs
/// only — no list, no blockquote, no tab, no `\r` (plan Gotcha G1).
const FIXTURE: &str = "# Title\n\nSome prose about café and 日本語のテスト mixed in.\n\nA second paragraph follows.\n";

fn key(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

fn shift() -> Mods {
    Mods {
        shift: true,
        ..Mods::NONE
    }
}

fn sup() -> Mods {
    Mods {
        sup: true,
        ..Mods::NONE
    }
}

fn sup_shift() -> Mods {
    Mods {
        sup: true,
        shift: true,
        ..Mods::NONE
    }
}

fn ctrl() -> Mods {
    Mods {
        ctrl: true,
        ..Mods::NONE
    }
}

/// One hand-written "normal human session": type prose, navigate, extend a
/// selection, copy, move, paste (via `ClipboardReply`, answering the
/// `CmdKind::ClipboardRead` a cmd+v keystroke arms), type more, undo x3,
/// redo x2, save + deliver, resize, type, arm a save failure, save +
/// deliver again, arm the ctrl+c quit chord and let it time out (a no-op —
/// G15 — so the session survives to keep typing).
fn tripwire_script() -> Vec<Action> {
    vec![
        // type prose
        Action::Type("annotate this: ".to_string()),
        // navigate
        Action::Key(key(KeyCode::Left, Mods::NONE)),
        Action::Key(key(KeyCode::Right, Mods::NONE)),
        Action::Key(key(KeyCode::Right, Mods::NONE)),
        Action::Key(key(KeyCode::Down, Mods::NONE)),
        Action::Key(key(KeyCode::End, Mods::NONE)),
        Action::Key(key(KeyCode::Home, Mods::NONE)),
        Action::Key(key(KeyCode::Up, Mods::NONE)),
        Action::Key(key(KeyCode::PageDown, Mods::NONE)),
        Action::Key(key(KeyCode::PageUp, Mods::NONE)),
        // shift-select
        Action::Key(key(KeyCode::Right, shift())),
        Action::Key(key(KeyCode::Right, shift())),
        Action::Key(key(KeyCode::Right, shift())),
        Action::Key(key(KeyCode::Left, shift())),
        // copy (cmd+c)
        Action::Key(key(KeyCode::Char('c'), sup())),
        // move
        Action::Key(key(KeyCode::Right, Mods::NONE)),
        // paste: cmd+v arms a ClipboardRead cmd; ClipboardReply answers it
        Action::Key(key(KeyCode::Char('v'), sup())),
        Action::ClipboardReply("pasted café 文字".to_string()),
        // type more
        Action::Type("more prose after the paste".to_string()),
        // undo x3
        Action::Key(key(KeyCode::Char('z'), sup())),
        Action::Key(key(KeyCode::Char('z'), sup())),
        Action::Key(key(KeyCode::Char('z'), sup())),
        // redo x2
        Action::Key(key(KeyCode::Char('z'), sup_shift())),
        Action::Key(key(KeyCode::Char('z'), sup_shift())),
        // cmd+s, then deliver its save
        Action::Key(key(KeyCode::Char('s'), sup())),
        Action::Deliver,
        // resize
        Action::Resize(100, 30),
        // type
        Action::Type("after the resize".to_string()),
        // arm a save failure, then save + deliver it
        Action::FailNextSave,
        Action::Key(key(KeyCode::Char('s'), sup())),
        Action::Deliver,
        // arm the ctrl+c quit chord, then let the confirm window time out
        Action::Key(key(KeyCode::Char('c'), ctrl())),
        Action::ConfirmTimeout,
        // type
        Action::Type("session continues after the timeout".to_string()),
    ]
}

/// WP4.S2 — a well-formed "normal human" session must trip nothing.
#[test]
fn clean_session_trips_nothing() {
    let result = driver::run(driver::DOC_PATH, FIXTURE, &tripwire_script());
    assert!(
        result.violation.is_none(),
        "{}",
        result
            .violation
            .as_ref()
            .map(|v| format!("{}: {}", v.id, v.message))
            .unwrap_or_default()
    );

    // Defeats a no-op driver: `violation.is_none()` alone is vacuously true
    // for a driver that never delivers anything (plan Gotcha G2's class of
    // vacuous gate). `Action::Type` expands unconditionally, one step per
    // `char`, with no gate on app state (`driver.rs`'s `for ch in
    // s.chars() { ... step_and_check(...) }`) — so the four `Type` actions
    // in `tripwire_script` alone floor the step count at the sum of their
    // literal lengths, independent of whether `Deliver`/`ConfirmTimeout`
    // happened to find nothing pending: "annotate this: ".len() (15) +
    // "more prose after the paste".len() (26) + "after the resize".len()
    // (16) + "session continues after the timeout".len() (35) = 92.
    // (Observed on this script: 121 steps — every other action also
    // contributes a step here — so 92 is a deliberately conservative
    // floor, not the true count.)
    assert!(
        result.steps >= 92,
        "expected at least 92 steps (one per Type char alone), got {}; \
         a driver that never delivers a Msg would report steps=0",
        result.steps
    );

    // Defeats a no-op driver: two no-ops trivially have equal, empty
    // content. The script types text and pastes via ClipboardReply, so the
    // buffer MUST differ from the seed; if it doesn't, that's a real
    // driver defect worth surfacing, not something to paper over here.
    assert_ne!(
        result.final_content, FIXTURE,
        "the script types and pastes text; final_content identical to the seed \
         means the driver did not actually deliver those edits"
    );
}

/// WP4.S5 — the driver is deterministic given `(content, actions)`: running
/// the same script twice must produce the same violation (none here) and
/// the same final content. This is the standing guarantee WP5's shrunk
/// scripts rely on to replay forever.
#[test]
fn driver_is_deterministic() {
    let script = tripwire_script();
    let first = driver::run(driver::DOC_PATH, FIXTURE, &script);
    let second = driver::run(driver::DOC_PATH, FIXTURE, &script);
    assert_eq!(
        first.violation, second.violation,
        "violation must be deterministic across two runs of the same script"
    );
    assert_eq!(
        first.final_content, second.final_content,
        "final content must be deterministic across two runs of the same script"
    );

    // Defeats a no-op driver: two runs of a driver that never delivers
    // anything are trivially "equal" (steps=0 == steps=0). Pinning
    // `steps > 0` as well as equality rules that out — determinism must be
    // demonstrated over real work, not over two identical no-ops.
    assert_eq!(
        first.steps, second.steps,
        "step count must be deterministic across two runs of the same script"
    );
    assert!(
        first.steps > 0,
        "expected the script to actually deliver messages, got steps=0"
    );
}

// ---------------------------------------------------------------------
// Hand-constructible `Snapshot` helpers for WP4.S3/S4. `Snapshot`'s fields
// are all `pub` (G16), so a checker's input is built directly with no need
// to drive the real `App` through anything.
// ---------------------------------------------------------------------

fn collapsed_cursor(id: u32, position: usize) -> Cursor {
    Cursor {
        position,
        anchor: position,
        desired_col: 0,
        id,
    }
}

fn selection_cursor(id: u32, anchor: usize, position: usize) -> Cursor {
    Cursor {
        position,
        anchor,
        desired_col: 0,
        id,
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
        save_in_flight: false,
        pending_quit: None,
        should_quit: false,
        status: String::new(),
        focus: Pane::Editor,
        modal_open: false,
        active: base_active_id(),
        read_only: false,
        cells: Vec::new(),
        row_meta: Vec::new(),
        highlight_spans: Vec::new(),
        highlight_version: 1,
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
fn cur_id_detects_zero() {
    let mut snap = base_snapshot("abcdefgh");
    snap.cursors = vec![collapsed_cursor(0, 0)];
    let v = cur_id(&snap).expect("a cursor with id=0 must trip CUR-ID");
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
