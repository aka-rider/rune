//! WP5/WP13 done-when: headless render assertions on a `TestBackend`,
//! using the `Mem` vfs — degenerate backend sizes and `blit`'s own
//! right-edge clipping. TODO.md's 500-line budget split of the original
//! `tui_render.rs`: conceal/styling/status-line/Cell-grid checks live in
//! `tui_render_basics.rs`, control-safe glyphs/tabs/graphemes in
//! `tui_render_text.rs`, and tables/the focus caret gate in
//! `tui_render_focus.rs`. The runtime loop itself is NOT exercised here
//! (plan: "test the pure update/view paths headlessly; do NOT spawn real
//! terminals in tests") — every test drives `App`/`render::draw` directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_render_common;

use rune_tui::render;
use rune_tui::testgrid;

use tui_render_common::{app_for, draw_into};

/// A 0x0 terminal (possible the instant a resize event lands before the
/// first real size is known, or a genuinely tiny/closing terminal) must not
/// panic — `render::draw`'s layout split and `blit`'s bounds checks must
/// degrade to "draw nothing" rather than index out of range.
#[test]
fn zero_by_zero_backend_does_not_panic() {
    let app = app_for("hello\n", 0, true);
    let _buf = draw_into(&app, 0, 0);
}

/// A 1x1 terminal must not panic either. At this size the status line's
/// `Constraint::Length(1)` consumes the entire height, leaving no room for
/// the editor viewport — exercising `blit`'s empty-area clipping path.
#[test]
fn one_by_one_backend_does_not_panic() {
    let app = app_for("hello\n", 0, true);
    let _buf = draw_into(&app, 1, 1);
}

/// WP13.S2 regression: `blit` must fits-check, not just start-check, a
/// wide `Cell`. A double-width glyph placed so it STARTS inside the area
/// but would need a column past `area`'s right edge (the border column,
/// one past the last column blit owns) must not touch that column at all
/// — `blit` should fall back to a blank single-width cell instead of
/// writing the glyph and letting its continuation spill over.
#[test]
fn blit_does_not_overpaint_past_the_right_edge_with_a_wide_glyph() {
    use ratatui::layout::Rect;

    let area = Rect::new(0, 0, 3, 1); // columns 0,1,2 owned by blit
    let narrow = |text: &str| render::Cell {
        text: text.into(),
        width: 1,
        style: ratatui::style::Style::default(),
        buf_offset: 0,
    };
    let wide = render::Cell {
        text: "\u{1F600}".into(), // U+1F600, width 2
        width: 2,
        style: ratatui::style::Style::default(),
        buf_offset: 0,
    };
    // "a" at x=0, "b" at x=1, wide glyph STARTS at x=2 (inside `area`, the
    // last owned column) but needs x=2..4 — one column past `right`(3).
    let rows = vec![vec![narrow("a"), narrow("b"), wide]];

    // Backend is one column wider than `area`; column 3 stands in for the
    // pane border blit must never touch.
    let buf = testgrid::draw_with(4, 1, |frame| render::blit(&rows, area, frame));

    assert_eq!(buf.cell((0, 0)).map(|c| c.symbol()), Some("a"));
    assert_eq!(buf.cell((1, 0)).map(|c| c.symbol()), Some("b"));
    assert_eq!(
        buf.cell((2, 0)).map(|c| c.symbol()),
        Some(" "),
        "the wide glyph doesn't fit in the last column of a 3-wide area — \
         blit must substitute a blank cell rather than the glyph"
    );
    assert_eq!(
        buf.cell((3, 0)).map(|c| c.symbol()),
        Some(" "),
        "column 3 is outside `area` entirely (the border column) and must \
         stay untouched/blank"
    );
}
