//! Shared test helpers for every submodule under `tests/invariants/` —
//! deliberately local copies of `tests/tripwire.rs`'s own helpers (an
//! integration test binary can't import another one; both build on
//! `Snapshot`/`Cursor`'s fully-`pub` fields, G16).

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::{Cursor, CursorSet};
use rune_fuzz::snapshot::Snapshot;
use rune_fuzz::step::{MsgTag, StepCtx};
use rune_tui::app::App;
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::render::Cell;
use rune_vfs::{Mem, Vfs};

pub(crate) fn key(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

pub(crate) fn sup() -> Mods {
    Mods {
        sup: true,
        ..Mods::NONE
    }
}

pub(crate) fn collapsed_cursor(id: u32, position: usize) -> Cursor {
    Cursor {
        position,
        anchor: position,
        desired_col: 0,
        id,
    }
}

pub(crate) fn selection_cursor(id: u32, anchor: usize, position: usize) -> Cursor {
    Cursor {
        position,
        anchor,
        desired_col: 0,
        id,
    }
}

/// Same derivation `Buffer` itself uses: starts at every byte right after
/// a `\n`; the last line's end is `content.len()`.
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
/// document as `DocumentId(NonZeroU64::MIN)` (`app.rs:170`), so this is a
/// deterministic constant in practice — a fresh in-memory `App` exists
/// only long enough to read `.active` back out of it.
pub(crate) fn base_active_id() -> DocumentId {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    App::new(Buffer::new(""), None, vfs, None).active
}

/// A second, definitely-different `DocumentId` from `base_active_id()`'s
/// value — `App::open_document` mints ids strictly increasing from
/// `NonZeroU64::MIN.saturating_add(1)` (`app.rs:176/207-208`), so calling
/// it once on a fresh `App` always returns something distinct from the
/// bootstrap document's own id. `PANE-NO-BLEED`'s "active document
/// changed" negative case (`tests/invariants/pane.rs`) needs exactly one
/// such value.
pub(crate) fn other_doc_id() -> DocumentId {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let mut app = App::new(Buffer::new(""), None, vfs, None);
    app.open_document(Buffer::new(""))
}

/// A well-formed baseline `Snapshot`: one valid collapsed cursor at offset
/// 0, a correctly derived line index, the editor focused with no modal up
/// (the precondition `PANE-NO-BLEED` and the undo/redo drive both assume),
/// otherwise-quiescent fields. Each test overrides exactly the field(s) it
/// exercises.
pub(crate) fn base_snapshot(content: &str) -> Snapshot {
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
        cells: Vec::new(),
    }
}

/// A neutral `StepCtx`: a `Resize` message (matches none of the L2
/// checkers' triggering patterns), no raw bytes, nothing on disk, no save
/// bookkeeping. Each test overrides exactly the field(s) it exercises.
pub(crate) fn base_ctx() -> StepCtx {
    StepCtx {
        step: 1,
        msg: MsgTag::Resize(80, 23),
        raw: Vec::new(),
        disk: None,
        pending_save_bytes: None,
        delivered_save_bytes: None,
        saves_delivered_ok: 0,
    }
}

pub(crate) fn cell(ch: char, buf_offset: i64) -> Cell {
    let theme = rune_tui::theme::Theme::catppuccin_mocha(false);
    Cell {
        text: ch.to_string(),
        width: 1,
        style: rune_tui::render::style_for(&theme, rune_syntax::ScopeId(0)),
        buf_offset,
    }
}

/// A real `WrapSnapshot` for `content` at `width`, built the same way
/// `rune-md/tests/wrap_roundtrip.rs`'s own `wrap_for` helper does.
pub(crate) fn wrap_for(content: &str, width: u16) -> (Buffer, rune_syntax::wrap::WrapSnapshot) {
    let buf = Buffer::new(content);
    let mut doc = rune_md::element::doc::DocMachine::new();
    doc.set_focus(true);
    doc.sync_content(&buf);
    let cursors = CursorSet::new(0);
    doc.sync_cursors(&buf, &cursors);
    let (lines, _snap) = rune_md::emit::emit(buf.content(), doc.blocks(), width);
    let wrap = rune_syntax::wrap::WrapMap::new(width).sync(buf.content(), &lines);
    (buf, wrap)
}
