//! WP5 done-when: headless render assertions on a `TestBackend`, using the
//! `Mem` vfs — the conceal-policy, styling, status-line, and raw `Cell`
//! grid checks. TODO.md's 500-line budget split of the original `tui_render.rs`:
//! control-safe glyphs/tabs/graphemes live in `tui_render_text.rs`,
//! degenerate backend sizes and `blit`'s own clipping in
//! `tui_render_bounds.rs`, and tables/the focus caret gate in
//! `tui_render_focus.rs`. The runtime loop itself is NOT exercised here
//! (plan: "test the pure update/view paths headlessly; do NOT spawn real
//! terminals in tests") — every test drives `App`/`render::draw` directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_render_common;

use rune_tui::render;

use tui_render_common::{
    EDITOR_TOP_ROW, HEIGHT, WIDTH, app_for, full_text, render_to_test_backend, row_text,
};

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
    let text = row_text(&buf, EDITOR_TOP_ROW, WIDTH);
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
    let cell = buf
        .cell((bold_start as u16, EDITOR_TOP_ROW))
        .expect("cell in bounds");
    assert!(
        cell.modifier.contains(ratatui::style::Modifier::BOLD),
        "expected the bold span's cell to carry the BOLD modifier"
    );
}

/// The footer row renders on the last row and shows its default-mode
/// content: a `GLOBAL_BINDINGS` hint on the left, `Ln n, Col n` on the
/// right (plan WP2.S6 — the file-name/dirty-dot placeholder this test used
/// to check for moved out of the footer; WP6's `title.rs` owns it next).
#[test]
fn status_line_present_on_last_row() {
    let app = app_for("hello\n", 0, true);
    let buf = render_to_test_backend(&app);
    let status_row = row_text(&buf, HEIGHT - 1, WIDTH);
    assert!(
        status_row.contains("explorer"),
        "expected a GLOBAL_BINDINGS hint on the footer row:\n{status_row}"
    );
    assert!(
        status_row.contains("Ln 1, Col 1"),
        "expected the cursor position on the footer row:\n{status_row}"
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
    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(&app, app.active_doc(), view);

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
