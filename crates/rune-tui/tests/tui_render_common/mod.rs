//! Shared setup helpers for the WP5 done-when headless render suite,
//! split across `tui_render_basics.rs` (conceal, styling, and the raw
//! `Cell` grid), `tui_render_text.rs` (control-safe glyphs, tabs, and
//! grapheme clusters), `tui_render_bounds.rs` (degenerate backend sizes
//! and `blit`'s own edge clipping), and `tui_render_focus.rs` (tables and
//! the focus/read-only caret gate) — TODO.md's 500-line budget split of the original
//! `tui_render.rs`. `tui_render_tables.rs` is a pre-existing sibling that
//! already followed this naming. Every consumer pulls this in via
//! `mod tui_render_common;` — integration test files are separate
//! binaries, so this is the one place all four draw an identical
//! `rune_fuzz::Session` fixture from, rather than risking drift.
#![allow(dead_code)]

use ratatui::buffer::Buffer as RtBuffer;

use rune_fuzz::Session;
use rune_tui::app::App;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::Effects;
use rune_tui::testgrid;

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

const RIGHT: KeyInput = KeyInput {
    code: KeyCode::Right,
    mods: Mods::NONE,
};

/// Walks the active document's caret from `Session::open`'s own byte 0 up to
/// `offset`, one `Right` press per grapheme step — the real navigation path,
/// never a `CursorSet::new` poke. Every caller's `offset` is a grapheme
/// boundary the document already carries (a `str::find` result or matching
/// arithmetic), so the caret's own position is guaranteed to land exactly on
/// it partway through the walk; the guard only catches a fixture that broke
/// that guarantee.
fn place_caret(session: &mut Session, offset: usize) {
    let target = offset.min(session.app().active_doc().buffer.content().len());
    let mut guard = 0usize;
    while session.app().active_doc().cursors.primary().position < target {
        session.key(RIGHT);
        guard += 1;
        assert!(
            guard <= target + 8,
            "caret placement stalled before reaching offset {target}"
        );
    }
}

/// `focused` no longer sets `Document::focused` directly (WP2: `App::
/// sync_view` derives it from `app.focus` every call — see its doc
/// comment) — an unfocused fixture instead moves `app.focus` off `Editor`
/// so the SAME derivation the real app uses produces `focused == false`.
/// Focus is gated on `LayoutMode` — show the column first so `Explorer` is
/// actually painted and the fixture keeps landing focus off the Editor as
/// intended. Neither has a dedicated `Session` verb, so this reaches
/// `app_mut()` for the same public `App` methods the pre-Session fixture
/// called directly, then `sync_view()`s — the focus change itself doesn't
/// go through `app::update`'s own post-dispatch sync, so a caller reading
/// conceal/caret state right after this would otherwise see it stale.
pub fn unfocus(session: &mut Session) {
    let mut effects = Effects::default();
    session.app_mut().splits.left.show();
    session
        .app_mut()
        .set_focus_pane(Pane::Explorer, &mut effects);
    session.app_mut().sync_view();
}

pub fn app_for(content: &str, cursor_offset: usize, focused: bool) -> Session {
    let mut session = Session::open("/doc.md", content);
    session.resize(WIDTH, HEIGHT);
    place_caret(&mut session, cursor_offset);
    if !focused {
        unfocus(&mut session);
    }
    session
}

/// Draws `app` into a `WIDTH`x`HEIGHT` `TestBackend` and hands back the raw
/// ratatui buffer — for callers that need cell-level style (color,
/// modifiers), which `Session::grid`/`row`'s plain-text rows discard. Goes
/// through `testgrid::draw`, the crate's sole `TestBackend` construction
/// site, exactly as `Session::grid`/`row` themselves do. Takes a plain
/// `&App` (via `session.app()`) rather than `&Session` so a fixture built
/// outside a `Session` can still draw through the same one place.
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
/// non-default terminal size (degenerate 0x0/1x1 sizes; `app_for`'s
/// WIDTH/HEIGHT is otherwise fixed).
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
