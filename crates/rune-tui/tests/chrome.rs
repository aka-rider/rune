//! WP2 "Done when" TestBackend integration tests: the left-pane geometry
//! (bordered Explorer/Open blocks, focus-colored border), `^x` toggling it
//! end-to-end through the real `app::update`, and the footer's `Ln n, Col
//! n` on a multiline buffer with a multibyte character (col in runes).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use ratatui::buffer::Buffer as RtBuffer;
use ratatui::style::Color;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg};
use rune_tui::testgrid;
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn app_for(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn draw(app: &App) -> RtBuffer {
    draw_with_width(app, WIDTH)
}

fn draw_with_width(app: &App, width: u16) -> RtBuffer {
    testgrid::draw(app, width, HEIGHT)
}

fn row_text(buf: &RtBuffer, y: u16, width: u16) -> String {
    let mut s = String::new();
    for x in 0..width {
        if let Some(cell) = buf.cell((x, y)) {
            s.push_str(cell.symbol());
        }
    }
    s
}

/// The `active_border`/`inactive_border` fg color of the cell at `(x, y)` —
/// `None` if that cell has no fg set at all (shouldn't happen on a border
/// cell, but a test failure here is more legible as a color mismatch than a
/// panic).
fn fg_at(buf: &RtBuffer, x: u16, y: u16) -> Option<Color> {
    buf.cell((x, y)).and_then(|c| c.style().fg)
}

/// `left_visible` with `focus == Explorer`: the top block's border (row 0,
/// the block's top edge) is the ACTIVE color; the bottom (Open Tabs) block's
/// border (the main area's last row, its own bottom edge) is INACTIVE.
#[test]
fn left_pane_shows_two_bordered_blocks_with_focus_colored_borders() {
    let mut app = app_for("hello");
    app.left_visible = true;
    app.focus = Pane::Explorer;
    app.sync_view();

    let buf = draw(&app);

    let explorer_top_border = fg_at(&buf, 1, 0).expect("explorer top border cell has a style");
    assert_eq!(
        explorer_top_border,
        app.theme.chrome.active_border.fg.unwrap(),
        "focused Explorer's border must be the ACTIVE color"
    );

    // The Open Tabs block's own bottom border sits on the main area's last
    // row (HEIGHT - 2: HEIGHT - 1 is the footer) regardless of exactly how
    // the 50/50 vertical split rounds.
    let tabs_bottom_border =
        fg_at(&buf, 1, HEIGHT - 2).expect("tabs bottom border cell has a style");
    assert_eq!(
        tabs_bottom_border,
        app.theme.chrome.inactive_border.fg.unwrap(),
        "unfocused Open Tabs' border must be the INACTIVE color"
    );

    let top_row = row_text(&buf, 0, WIDTH);
    assert!(top_row.contains("Files"), "expected the Files pane title");
}

/// The editor pane's geometry is unchanged when `left_visible` is false —
/// no left column, no borders, full-width editor (plan WP2.S5).
#[test]
fn left_pane_hidden_by_default_leaves_editor_geometry_unchanged() {
    let app = app_for("hello");
    assert!(!app.left_visible);
    let buf = draw(&app);
    let top_row = row_text(&buf, 0, WIDTH);
    assert!(
        !top_row.contains("Files"),
        "no left pane chrome expected when left_visible is false:\n{top_row}"
    );
}

/// `^b` end-to-end through the real `app::update`: flips `left_visible`,
/// focuses the Explorer, and the very next render shows the bordered
/// blocks (plan WP2.S7: "ToggleExplorer flips left_visible+focus"; plan
/// WP5.S1: `^b` is the always-works ctrl fallback for the Explorer, `^x`
/// having retired in favor of the held-space leader's `␣x`).
#[test]
fn ctrl_b_toggles_the_explorer_through_update() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('b'),
            mods: Mods {
                ctrl: true,
                ..Mods::NONE
            },
        }),
        &mut effects,
    );
    assert!(app.left_visible);
    assert_eq!(app.focus, Pane::Explorer);

    app.sync_view();
    let buf = draw(&app);
    assert!(row_text(&buf, 0, WIDTH).contains("Files"));
}

/// Plan WP6.S7/risk R3: with the Explorer focused (the contextual hints
/// that can now grow past `GLOBAL_BINDINGS`' old fixed set), `Ln n, Col n`
/// must still be visible on the footer row at both a full-width (80) and a
/// narrow (40) terminal — the priority truncation reserves room for it
/// first, so it can never fall off regardless of how many hint entries fit.
#[test]
fn footer_position_readout_survives_truncation_at_narrow_widths() {
    for width in [80u16, 40u16] {
        let mut app = app_for("hello");
        app.left_visible = true;
        app.focus = Pane::Explorer;
        app.sync_view();

        let buf = draw_with_width(&app, width);
        let footer_row = row_text(&buf, HEIGHT - 1, width);
        assert!(
            footer_row.contains("Ln 1, Col 1"),
            "width {width}: expected 'Ln 1, Col 1' on the footer row:\n{footer_row}"
        );
    }
}

/// `Ln n, Col n` on a multiline buffer with a multibyte character before
/// the cursor: the column counts RUNES, not bytes (§1.5, Assumption A3).
#[test]
fn footer_reports_rune_column_on_a_multiline_multibyte_buffer() {
    let mut app = app_for("first\nab\u{6c49}cd");
    let id = app.active;
    // Byte offset right after "first\nab\u{6c49}" — 3 runes into line 2
    // ('a','b','\u{6c49}'), so the display column is 4.
    let offset = "first\nab\u{6c49}".len();
    app.doc_mut(id).unwrap().cursors = CursorSet::new(offset);
    app.sync_view();

    let buf = draw(&app);
    let footer_row = row_text(&buf, HEIGHT - 1, WIDTH);
    assert!(
        footer_row.contains("Ln 2, Col 4"),
        "expected 'Ln 2, Col 4' (rune column, not byte column) on the footer row:\n{footer_row}"
    );
}
