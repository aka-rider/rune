//! Headless render assertions on a `TestBackend`, using the
//! `Mem` vfs — control-safe glyphs and tab expansion. This is the
//! 500-line-budget split of the original `tui_render.rs`: grapheme-cluster
//! cells live in the sibling `tui_render_graphemes.rs`; conceal/
//! styling/status-line/Cell-grid checks live in `tui_render_basics.rs`,
//! degenerate backend sizes and `blit`'s own clipping in
//! `tui_render_bounds.rs`, and tables/the focus caret gate in
//! `tui_render_focus.rs`. The runtime loop itself is NOT exercised here
//! (test the pure update/view paths headlessly; do NOT spawn real
//! terminals in tests) — every test drives `App`/`render::draw` directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_render_common;

use rune_tui::render;

use tui_render_common::{
    EDITOR_LEFT_COL, EDITOR_TOP_ROW, HEIGHT, WIDTH, app_for, caret_column, full_text,
    render_to_test_backend, row_text,
};

/// Regression for the control-safe cell builder: `\r` (from a CRLF file —
/// it must stay in the buffer verbatim) must never
/// become a `Cell`, and rendering it must not panic. Before the fix, a raw
/// `\r` reached `ratatui::buffer::Cell::set_char`, and `cell_width()`
/// `debug_assert!`s on any single-byte ASCII control character reaching a
/// cell — this test IS the regression check: merely rendering CRLF content
/// without panicking is the assertion.
#[test]
fn crlf_line_endings_render_without_panicking_and_leave_no_control_chars_in_cells() {
    let content = "ab\r\ncd\r\n";
    let session = app_for(content, 0, true);
    let app = session.app();

    let buf = render_to_test_backend(app);
    // `full_text` itself joins rows with '\n' as a formatting separator, so
    // only '\r' is checked here — a real leaked '\n' cell is instead caught
    // below, directly on the `Cell` grid (which has no such separator).
    let text = full_text(&buf, HEIGHT, WIDTH);
    assert!(
        !text.contains('\r'),
        "a raw CR must never reach the terminal buffer:\n{text:?}"
    );
    assert!(text.contains("ab"), "expected 'ab' visible:\n{text}");
    assert!(text.contains("cd"), "expected 'cd' visible:\n{text}");

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
    for row in &rows {
        for cell in row {
            assert!(
                !matches!(cell.text.as_str(), "\r" | "\n"),
                "a raw CR/LF must never become a Cell: {cell:?}"
            );
        }
    }
}

/// Sibling to the CRLF regression above, for the case CRLF does NOT cover:
/// a LONE `\r` (no paired `\n`) is ordinary mid-line content to the buffer
/// (never a line break), so `"ab\rcd\r"` is a single
/// buffer line containing two literal `\r` bytes. This must render without
/// panicking and without ever letting a raw `\r` reach a `Cell`, exactly
/// like the CRLF case — the render-layer contract (the user's
/// bytes stay in the buffer verbatim; the control-safe cell builder maps
/// a control byte to a placeholder glyph, never a raw `Cell`) does not
/// distinguish a CR paired with LF from one that stands alone.
#[test]
fn lone_cr_line_endings_render_without_panicking_and_leave_no_control_chars_in_cells() {
    let content = "ab\rcd\r";
    let session = app_for(content, 0, true);
    let app = session.app();

    let buf = render_to_test_backend(app);
    let text = full_text(&buf, HEIGHT, WIDTH);
    assert!(
        !text.contains('\r'),
        "a raw CR must never reach the terminal buffer:\n{text:?}"
    );
    assert!(text.contains("ab"), "expected 'ab' visible:\n{text}");
    assert!(text.contains("cd"), "expected 'cd' visible:\n{text}");

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
    for row in &rows {
        for cell in row {
            assert!(
                !matches!(cell.text.as_str(), "\r" | "\n"),
                "a raw CR/LF must never become a Cell: {cell:?}"
            );
        }
    }
}

/// Regression for the unified width chokepoint: a tab mid-line must expand
/// to the SAME next-4-stop column both `render::segment_cells` and
/// `WrapSnapshot::visual_col` compute, so the caret lands on the character
/// after the tab, not one column short of it. Before the fix, the render
/// side treated a tab as width 1 (via `control_aware_width` alone) while
/// wrap's `visual_col` used `rune_width_with_tab`'s 4-stop math — the caret
/// landed mid-tab-expansion instead of on "c".
#[test]
fn tab_caret_column_agrees_with_wrap_visual_col() {
    let content = "ab\tcd\n";
    let cursor_offset = 3; // byte offset of 'c', right after the tab
    let session = app_for(content, cursor_offset, true);
    let app = session.app();

    let view = app.active_doc().view.as_ref().expect("synced view");
    let buffer_point = app.active_doc().buffer.offset_to_line_col(cursor_offset);
    let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
    let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
    let expected_visual_col = view
        .wrap
        .visual_col(content, wrap_point.row, wrap_point.col);
    assert_eq!(
        expected_visual_col, 4,
        "a tab starting at column 2 must expand to the next 4-stop (column 4)"
    );

    let buf = render_to_test_backend(app);
    // Skip the center block's left AND right border columns (plan gotcha
    // 10) before comparing against the editor-relative text.
    let text: String = row_text(&buf, EDITOR_TOP_ROW, WIDTH)
        .chars()
        .skip(EDITOR_LEFT_COL as usize)
        .collect();
    assert_eq!(
        text.trim_end_matches('│').trim_end(),
        "ab  cd",
        "the tab must expand to exactly 2 columns here"
    );

    let caret_x = caret_column(&buf, EDITOR_TOP_ROW, WIDTH)
        .expect("caret cell must be present on the editor's first row");
    assert_eq!(
        (caret_x - EDITOR_LEFT_COL) as usize,
        expected_visual_col,
        "caret column must agree with wrap's visual_col across a tab"
    );
}

/// Wide-char (CJK, width 2) followed by a tab: the tab's 4-stop math must
/// key off the ACCUMULATED visual column (2, after the wide char), not the
/// char count (1) — and the caret must still agree with `visual_col`.
#[test]
fn wide_char_then_tab_caret_column_agrees_with_wrap_visual_col() {
    let content = "\u{6c49}\tab\n"; // U+6C49 (汉, width 2), tab, "ab"
    let cursor_offset = 4; // byte offset of 'a': 3 bytes of 汉 + 1 byte tab
    let session = app_for(content, cursor_offset, true);
    let app = session.app();

    let view = app.active_doc().view.as_ref().expect("synced view");
    let buffer_point = app.active_doc().buffer.offset_to_line_col(cursor_offset);
    let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
    let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
    let expected_visual_col = view
        .wrap
        .visual_col(content, wrap_point.row, wrap_point.col);
    assert_eq!(
        expected_visual_col, 4,
        "汉 (width 2) then a tab to the next 4-stop must land 'a' at column 4"
    );

    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
    let first_row = rows.first().expect("at least one row");
    assert_eq!(
        first_row.first().map(|c| (c.text.as_str(), c.width)),
        Some(("\u{6c49}", 2))
    );
    let tab_cells: Vec<_> = first_row
        .iter()
        .skip(1)
        .take_while(|c| c.text == " " && c.buf_offset == Some(3))
        .collect();
    assert_eq!(
        tab_cells.len(),
        2,
        "the tab (starting at visual col 2) must expand to exactly 2 single-width cells: {first_row:?}"
    );

    let buf = render_to_test_backend(app);
    let caret_x = caret_column(&buf, EDITOR_TOP_ROW, WIDTH)
        .expect("caret cell must be present on the editor's first row");
    assert_eq!((caret_x - EDITOR_LEFT_COL) as usize, expected_visual_col);
}

/// Regression for the control-safe cell builder: a non-tab/newline control
/// character (BEL, `\x07`) must never reach `ratatui::buffer::Cell::set_char`
/// either — it gets the Unicode "control picture" placeholder (`U+2407`)
/// instead, at the control char's own `buf_offset`.
#[test]
fn control_char_gets_a_safe_placeholder_glyph() {
    let content = "a\u{7}b\n";
    let session = app_for(content, 0, true);
    let app = session.app();

    let buf = render_to_test_backend(app);
    let text = full_text(&buf, HEIGHT, WIDTH);
    assert!(
        !text.contains('\u{7}'),
        "a raw BEL must never reach the terminal buffer:\n{text:?}"
    );
    assert!(
        text.contains('\u{2407}'),
        "expected the BEL control-picture placeholder (U+2407):\n{text:?}"
    );

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
    let placeholder = rows
        .first()
        .and_then(|row| row.iter().find(|c| c.text == "\u{2407}"))
        .expect("placeholder cell present in row 0");
    assert_eq!(
        placeholder.buf_offset,
        Some(1),
        "the BEL is the 2nd byte (offset 1) of \"a\\x07b\""
    );
}
