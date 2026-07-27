//! Shared test helpers for every submodule under `tests/invariants/` —
//! deliberately local copies of `tests/tripwire.rs`'s own helpers (an
//! integration test binary can't import another one; both build on
//! `Snapshot`/`Cursor`'s fully-`pub` fields, G16).

use rune_core::buffer::Buffer;
use rune_core::cursor::{Cursor, CursorSet};
use rune_fuzz::snapshot::Snapshot;
use rune_fuzz::step::{MsgTag, StepCtx};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::render::Cell;

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

/// A well-formed baseline `Snapshot`: one valid collapsed cursor at offset
/// 0, a correctly derived line index, otherwise-quiescent fields. Each
/// test overrides exactly the field(s) it exercises.
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
    Cell {
        ch,
        width: 1,
        style: rune_tui::render::style_for(rune_md::emit::StyleId::Text),
        buf_offset,
    }
}

/// A real `WrapSnapshot` for `content` at `width`, built the same way
/// `rune-md/tests/wrap_roundtrip.rs`'s own `wrap_for` helper does.
pub(crate) fn wrap_for(content: &str, width: u16) -> (Buffer, rune_md::wrap::WrapSnapshot) {
    let buf = Buffer::new(content);
    let mut doc = rune_md::element::doc::DocMachine::new();
    doc.set_focus(true);
    doc.sync_content(&buf);
    let cursors = CursorSet::new(0);
    doc.sync_cursors(&buf, &cursors);
    let (lines, _snap) = rune_md::emit::emit(buf.content(), doc.blocks());
    let wrap = rune_md::wrap::WrapMap::new(width).sync(&lines);
    (buf, wrap)
}
