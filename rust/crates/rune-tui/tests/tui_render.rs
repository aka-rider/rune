//! WP5 done-when: headless render assertions on a `TestBackend`, using the
//! `Mem` vfs. The runtime loop itself is NOT exercised here (plan: "test the
//! pure update/view paths headlessly; do NOT spawn real terminals in
//! tests") — every test drives `App`/`render::draw` directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer as RtBuffer;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_core::vfs::Mem;
use rune_tui::app::App;
use rune_tui::render;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn app_for(content: &str, cursor_offset: usize, focused: bool) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()));
    app.editor.focused = focused;
    app.editor.cursors = CursorSet::new(cursor_offset.min(content.len()));
    app.editor.viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn render_to_test_backend(app: &App) -> RtBuffer {
    let backend = TestBackend::new(WIDTH, HEIGHT);
    let mut terminal = Terminal::new(backend).expect("terminal construction");
    terminal
        .draw(|frame| render::draw(app, frame))
        .expect("draw");
    terminal.backend().buffer().clone()
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

fn full_text(buf: &RtBuffer, height: u16, width: u16) -> String {
    let mut s = String::new();
    for y in 0..height {
        s.push_str(&row_text(buf, y, width));
        s.push('\n');
    }
    s
}

/// A concealed heading marker never shows when the cursor is elsewhere on
/// the document — the `Decide` reveal policy for `Heading` is
/// `cursors.any_on_line(line)` (plan Context, reveal-policy table).
#[test]
fn concealed_heading_marker_not_visible_when_cursor_elsewhere() {
    let content = "## Heading\n\nSome text below.\n";
    // Cursor on line 2 ("Some text below."), not on the heading's line 0.
    let cursor_offset = content.find("Some").expect("fixture contains 'Some'");
    let app = app_for(content, cursor_offset, true);

    let buf = render_to_test_backend(&app);
    let text = full_text(&buf, HEIGHT, WIDTH);

    assert!(
        !text.contains("## "),
        "heading marker must be concealed when the cursor is elsewhere:\n{text}"
    );
    assert!(
        text.contains("Heading"),
        "the heading's own text must still render:\n{text}"
    );
}

/// Unfocused forces every reveal-`Decide` element concealed regardless of
/// cursor position (plan Gotchas: "Unfocused -> ForceRendered").
#[test]
fn concealed_heading_marker_not_visible_when_unfocused_even_with_cursor_on_it() {
    let content = "## Heading\n";
    let app = app_for(content, 0, false); // cursor ON the heading line, but unfocused

    let buf = render_to_test_backend(&app);
    let text = full_text(&buf, HEIGHT, WIDTH);

    assert!(
        !text.contains("## "),
        "unfocused must conceal the heading marker even with the cursor on its line:\n{text}"
    );
}

/// Bold text renders with the `BOLD` modifier — `StyleId::Bold` ->
/// `render::style_for`.
#[test]
fn bold_text_is_styled_bold() {
    let content = "plain **bold** plain\n";
    // Cursor away from the emphasis span so it stays concealed/rendered
    // (folded), not revealed with its `**` delimiters.
    let app = app_for(content, 0, true);

    let buf = render_to_test_backend(&app);
    let text = row_text(&buf, 0, WIDTH);
    assert!(
        text.contains("bold"),
        "expected folded bold text visible:\n{text}"
    );
    assert!(
        !text.contains("**"),
        "bold delimiters must stay concealed:\n{text}"
    );

    // Find the cell under the 'b' of "bold" and check its style carries BOLD.
    let bold_start = text.find("bold").expect("bold text present");
    let cell = buf.cell((bold_start as u16, 0)).expect("cell in bounds");
    assert!(
        cell.modifier.contains(ratatui::style::Modifier::BOLD),
        "expected the bold span's cell to carry the BOLD modifier"
    );
}

/// The status line renders on the last row and shows the (unnamed) file
/// name placeholder.
#[test]
fn status_line_present_on_last_row() {
    let app = app_for("hello\n", 0, true);
    let buf = render_to_test_backend(&app);
    let status_row = row_text(&buf, HEIGHT - 1, WIDTH);
    assert!(
        status_row.contains("[No Name]"),
        "expected the unnamed-draft placeholder on the status row:\n{status_row}"
    );
}

/// `render::build_rows`' `Cell` grid: every visible char's `buf_offset`
/// either matches a real position in the source text or is a documented
/// synthetic cell — asserted directly on the `Cell` grid, not just the
/// backend buffer's text (plan WP5.S3: "assertions on ... your Cell grid
/// (buf_offset mapping)").
#[test]
fn cell_grid_buf_offsets_map_back_into_the_source_text() {
    let content = "## Heading\n";
    let app = app_for(content, 0, true); // cursor on the heading line: revealed
    let view = app.view.as_ref().expect("synced view");
    let rows = render::build_rows(view, &app);

    let first_row = rows.first().expect("at least one row");
    assert!(
        !first_row.is_empty(),
        "revealed heading row must have cells"
    );

    for cell in first_row {
        if cell.buf_offset < 0 {
            continue; // decorative/synthetic — not required to map back
        }
        let offset = cell.buf_offset as usize;
        assert!(
            offset <= content.len(),
            "buf_offset {offset} exceeds source length {}",
            content.len()
        );
        // Every mapped offset must land on a UTF-8 char boundary.
        assert!(
            content.is_char_boundary(offset),
            "buf_offset {offset} is not a char boundary in {content:?}"
        );
    }

    // Revealed heading marker: the very first cell should map back to byte
    // 0 (the '#' of "## Heading").
    assert_eq!(first_row.first().map(|c| c.buf_offset), Some(0));
}
