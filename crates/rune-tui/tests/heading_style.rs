//! WP1.S5: the concealed-heading styling fix (`markup.heading.N` reaching a
//! Rendered heading's own cells, not just a Revealed one's) pinned end to
//! end through the real render pipeline. Lives in its own file because the
//! main render test file is already over the §1.6 size budget; the small
//! `testgrid`/`app_for` helpers are duplicated locally, following this
//! crate's established pattern of each integration-test binary keeping its
//! own copy rather than sharing helpers across binaries.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use ratatui::buffer::Buffer as RtBuffer;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_syntax::scope::scope_table;
use rune_tui::app::App;
use rune_tui::pane::Pane;
use rune_tui::runtime::Effects;
use rune_tui::testgrid;
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

/// First editor content row: the rows above it are pane chrome (title and
/// breadcrumb), the same accounting the other render tests pin.
const EDITOR_TOP_ROW: u16 = 2;

fn app_for(content: &str, cursor_offset: usize, focused: bool) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    if !focused {
        // Focus is gated on `LayoutMode` — show the column first so
        // `Explorer` is actually painted and the fixture keeps landing
        // focus off the Editor as intended.
        app.splits.left.show();
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

fn render(app: &App) -> RtBuffer {
    testgrid::draw(app, WIDTH, HEIGHT)
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

/// The backend COLUMN (not a byte offset into the joined row string —
/// `row_text`'s multi-byte border glyphs make those two disagree) at which
/// `needle`'s cell-by-cell symbol sequence starts on row `y`, scanning cell
/// by cell rather than through a concatenated string.
fn find_col(buf: &RtBuffer, y: u16, width: u16, needle: &str) -> Option<u16> {
    let want: Vec<&str> = needle.split("").filter(|s| !s.is_empty()).collect();
    (0..width).find(|&x| {
        want.iter().enumerate().all(|(i, sym)| {
            buf.cell((x + i as u16, y))
                .is_some_and(|cell| cell.symbol() == *sym)
        })
    })
}

/// The Rendered (concealed-marker) branch of a heading must style its
/// inline content `markup.heading.1`, not the plain-text default — the
/// root bug this work package fixes: before it, the Rendered branch called
/// `emit_inlines` with `StyleCtx::default()`, so `markup.heading.N` was
/// only ever reachable when the line was fully Revealed.
#[test]
fn concealed_heading_content_carries_the_heading_style() {
    let content = "# Title\n\ntext\n";
    // Cursor away from the heading's own line so it stays Rendered
    // (marker concealed) rather than Revealed.
    let cursor = content.find("text").expect("fixture has a text line");
    let app = app_for(content, cursor, true);

    let buf = render(&app);
    let text = row_text(&buf, EDITOR_TOP_ROW, WIDTH);
    assert!(
        !text.contains("# "),
        "the heading marker must still be concealed:\n{text}"
    );
    let title_start = find_col(&buf, EDITOR_TOP_ROW, WIDTH, "Title").expect("heading text renders");

    let expected = app.theme.scope_style(
        scope_table()
            .resolve("markup.heading.1")
            .expect("known scope"),
    );
    for i in 0u16..5 {
        let x = title_start + i;
        let cell = buf
            .cell((x, EDITOR_TOP_ROW))
            .expect("cell in bounds for heading text");
        assert_eq!(
            cell.style().fg,
            expected.fg,
            "heading cell at column {x} must carry markup.heading.1's fg"
        );
        assert_eq!(
            cell.modifier, expected.add_modifier,
            "heading cell at column {x} must carry markup.heading.1's modifiers"
        );
    }
}

/// A3 (decided): in-heading emphasis is flattened while concealed — the
/// Rendered branch emits the whole heading uniformly via `StyleCtx::
/// Override`, discarding a nested `markup.strong`/`markup.italic`, exactly
/// like the Revealed branch already does for the whole line. This test
/// pins that the loss is deliberate, not accidental: every cell across
/// "bold" carries the SAME heading style, no bold modifier surviving from
/// the emphasis markup underneath.
#[test]
fn concealed_heading_flattens_nested_emphasis_to_the_heading_style() {
    let content = "# Some **bold** title\n\ntext\n";
    let cursor = content.rfind("text").expect("fixture has a text line");
    let app = app_for(content, cursor, true);

    let buf = render(&app);
    let text = row_text(&buf, EDITOR_TOP_ROW, WIDTH);
    assert!(
        !text.contains("# ") && !text.contains("**"),
        "heading marker and emphasis delimiters must stay concealed:\n{text}"
    );
    assert!(
        text.contains("Some bold title"),
        "folded emphasis text must still render:\n{text}"
    );

    let expected = app.theme.scope_style(
        scope_table()
            .resolve("markup.heading.1")
            .expect("known scope"),
    );
    let bold_start = find_col(&buf, EDITOR_TOP_ROW, WIDTH, "bold").expect("folded bold renders");
    for i in 0u16..4 {
        let x = bold_start + i;
        let cell = buf
            .cell((x, EDITOR_TOP_ROW))
            .expect("cell in bounds for the flattened emphasis span");
        assert_eq!(
            cell.style().fg,
            expected.fg,
            "the emphasis span must be flattened to markup.heading.1's fg, not styled bold separately"
        );
        assert_eq!(
            cell.modifier, expected.add_modifier,
            "the emphasis span must not carry its own BOLD modifier on top of the heading's"
        );
    }
}
