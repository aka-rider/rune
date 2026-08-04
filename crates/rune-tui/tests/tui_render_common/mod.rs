//! Shared setup helpers for the WP5 done-when headless render suite,
//! split across `tui_render_basics.rs` (conceal, styling, and the raw
//! `Cell` grid), `tui_render_text.rs` (control-safe glyphs, tabs, and
//! grapheme clusters), `tui_render_bounds.rs` (degenerate backend sizes
//! and `blit`'s own edge clipping), and `tui_render_focus.rs` (tables and
//! the focus/read-only caret gate) — TODO.md's §1.6 split of the original
//! `tui_render.rs`. `tui_render_tables.rs` is a pre-existing sibling that
//! already followed this naming. Every consumer pulls this in via
//! `mod tui_render_common;` — integration test files are separate
//! binaries, so this is the one place all four draw an identical `App`
//! fixture from, rather than risking drift.
#![allow(dead_code)]

use std::sync::Arc;

use ratatui::buffer::Buffer as RtBuffer;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::App;
use rune_tui::pane::Pane;
use rune_tui::runtime::Effects;
use rune_tui::testgrid;
use rune_vfs::Mem;

pub const WIDTH: u16 = 80;
pub const HEIGHT: u16 = 24;

/// The editor's own first row within the full backend (plan WP6.S2: the
/// center pane reserves a title row + a breadcrumb row above the editor
/// whenever it's tall enough — `app_for`'s fixed HEIGHT always is). Tests
/// that pin an assertion to a specific editor row use this rather than a
/// bare literal `0`, so a future chrome-row change has one place to update.
/// Stays `2` after WP4 (plan gotcha 10): row 0 is now the top border, row 1
/// the title, row 2 the editor's first content row — same literal value,
/// different provenance.
pub const EDITOR_TOP_ROW: u16 = 2;

/// The editor content's first COLUMN within the full backend (plan gotcha
/// 10): WP4's center `Block::bordered()` puts a `│` at column 0, so the
/// editor's own column 0 (where `WrapSnapshot::visual_col` starts counting)
/// is backend column 1, not 0. Any assertion comparing a backend column
/// against a `visual_col`/wrap-relative column must offset by this.
pub const EDITOR_LEFT_COL: u16 = 1;

/// `focused` no longer sets `Document::focused` directly (WP2: `App::
/// sync_view` derives it from `app.focus` every call — see its doc
/// comment) — an unfocused fixture instead moves `app.focus` off `Editor`
/// so the SAME derivation the real app uses produces `focused == false`.
pub fn app_for(content: &str, cursor_offset: usize, focused: bool) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    if !focused {
        app.set_focus_pane(Pane::Explorer, &mut Effects::default());
    }
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor_offset.min(content.len()));
    app.doc_mut(id)
        .unwrap()
        .viewport
        .set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

pub fn render_to_test_backend(app: &App) -> RtBuffer {
    testgrid::draw(app, WIDTH, HEIGHT)
}

pub fn row_text(buf: &RtBuffer, y: u16, width: u16) -> String {
    let mut s = String::new();
    for x in 0..width {
        if let Some(cell) = buf.cell((x, y)) {
            s.push_str(cell.symbol());
        }
    }
    s
}

pub fn full_text(buf: &RtBuffer, height: u16, width: u16) -> String {
    let mut s = String::new();
    for y in 0..height {
        s.push_str(&row_text(buf, y, width));
        s.push('\n');
    }
    s
}

/// Renders `app` into a `w`x`h` `TestBackend` — for tests that need a
/// non-default terminal size (degenerate 0x0/1x1 sizes; `app_for`'s WIDTH/
/// HEIGHT is otherwise fixed).
pub fn draw_into(app: &App, w: u16, h: u16) -> RtBuffer {
    testgrid::draw(app, w, h)
}

/// The backend column of the cell carrying the cursor's reverse-video
/// overlay on row `y`, or `None` if no cell on that row is reversed. Since
/// `render::blit` advances its backend `x` by each `Cell`'s `width` (not by
/// 1), this backend column IS the visual column — the same space
/// `WrapSnapshot::visual_col` computes into.
pub fn caret_column(buf: &RtBuffer, y: u16, width: u16) -> Option<u16> {
    (0..width).find(|&x| {
        buf.cell((x, y))
            .is_some_and(|c| c.modifier.contains(ratatui::style::Modifier::REVERSED))
    })
}
