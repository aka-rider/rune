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
use rune_tui::app::App;
use rune_tui::pane::Pane;
use rune_tui::render;
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

/// The editor's own first row within the full backend (plan WP6.S2: the
/// center pane reserves a title row + a breadcrumb row above the editor
/// whenever it's tall enough — `app_for`'s fixed HEIGHT always is). Tests
/// that pin an assertion to a specific editor row use this rather than a
/// bare literal `0`, so a future chrome-row change has one place to update.
const EDITOR_TOP_ROW: u16 = 2;

/// `focused` no longer sets `Document::focused` directly (WP2: `App::
/// sync_view` derives it from `app.focus` every call — see its doc
/// comment) — an unfocused fixture instead moves `app.focus` off `Editor`
/// so the SAME derivation the real app uses produces `focused == false`.
fn app_for(content: &str, cursor_offset: usize, focused: bool) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    if !focused {
        app.focus = Pane::Explorer;
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

/// Renders `app` into a `w`x`h` `TestBackend` — for tests that need a
/// non-default terminal size (degenerate 0x0/1x1 sizes; `app_for`'s WIDTH/
/// HEIGHT is otherwise fixed).
fn draw_into(app: &App, w: u16, h: u16) -> RtBuffer {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("terminal construction");
    terminal
        .draw(|frame| render::draw(app, frame))
        .expect("draw");
    terminal.backend().buffer().clone()
}

/// The backend column of the cell carrying the cursor's reverse-video
/// overlay on row `y`, or `None` if no cell on that row is reversed. Since
/// `render::blit` advances its backend `x` by each `Cell`'s `width` (not by
/// 1), this backend column IS the visual column — the same space
/// `WrapSnapshot::visual_col` computes into.
fn caret_column(buf: &RtBuffer, y: u16, width: u16) -> Option<u16> {
    (0..width).find(|&x| {
        buf.cell((x, y))
            .is_some_and(|c| c.modifier.contains(ratatui::style::Modifier::REVERSED))
    })
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

/// Regression for the control-safe cell builder: `\r` (from a CRLF file —
/// CONSTITUTION §1.4.5 requires it stay in the buffer verbatim) must never
/// become a `Cell`, and rendering it must not panic. Before the fix, a raw
/// `\r` reached `ratatui::buffer::Cell::set_char`, and `cell_width()`
/// `debug_assert!`s on any single-byte ASCII control character reaching a
/// cell — this test IS the regression check: merely rendering CRLF content
/// without panicking is the assertion.
#[test]
fn crlf_line_endings_render_without_panicking_and_leave_no_control_chars_in_cells() {
    let content = "ab\r\ncd\r\n";
    let app = app_for(content, 0, true);

    let buf = render_to_test_backend(&app);
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
    let rows = render::build_rows(view, &app);
    for row in &rows {
        for cell in row {
            assert!(
                !matches!(cell.ch, '\r' | '\n'),
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
    let app = app_for(content, cursor_offset, true);

    let view = app.active_doc().view.as_ref().expect("synced view");
    let buffer_point = app.active_doc().buffer.offset_to_line_col(cursor_offset);
    let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
    let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
    let expected_visual_col = view.wrap.visual_col(wrap_point.row, wrap_point.col);
    assert_eq!(
        expected_visual_col, 4,
        "a tab starting at column 2 must expand to the next 4-stop (column 4)"
    );

    let buf = render_to_test_backend(&app);
    let text = row_text(&buf, EDITOR_TOP_ROW, WIDTH);
    assert_eq!(
        text.trim_end(),
        "ab  cd",
        "the tab must expand to exactly 2 columns here"
    );

    let caret_x = caret_column(&buf, EDITOR_TOP_ROW, WIDTH)
        .expect("caret cell must be present on the editor's first row");
    assert_eq!(
        caret_x as usize, expected_visual_col,
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
    let app = app_for(content, cursor_offset, true);

    let view = app.active_doc().view.as_ref().expect("synced view");
    let buffer_point = app.active_doc().buffer.offset_to_line_col(cursor_offset);
    let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
    let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
    let expected_visual_col = view.wrap.visual_col(wrap_point.row, wrap_point.col);
    assert_eq!(
        expected_visual_col, 4,
        "汉 (width 2) then a tab to the next 4-stop must land 'a' at column 4"
    );

    let rows = render::build_rows(view, &app);
    let first_row = rows.first().expect("at least one row");
    assert_eq!(
        first_row.first().map(|c| (c.ch, c.width)),
        Some(('\u{6c49}', 2))
    );
    let tab_cells: Vec<_> = first_row
        .iter()
        .skip(1)
        .take_while(|c| c.ch == ' ' && c.buf_offset == 3)
        .collect();
    assert_eq!(
        tab_cells.len(),
        2,
        "the tab (starting at visual col 2) must expand to exactly 2 single-width cells: {first_row:?}"
    );

    let buf = render_to_test_backend(&app);
    let caret_x = caret_column(&buf, EDITOR_TOP_ROW, WIDTH)
        .expect("caret cell must be present on the editor's first row");
    assert_eq!(caret_x as usize, expected_visual_col);
}

/// Regression for the control-safe cell builder: a non-tab/newline control
/// character (BEL, `\x07`) must never reach `ratatui::buffer::Cell::set_char`
/// either — it gets the Unicode "control picture" placeholder (`U+2407`)
/// instead, at the control char's own `buf_offset`.
#[test]
fn control_char_gets_a_safe_placeholder_glyph() {
    let content = "a\u{7}b\n";
    let app = app_for(content, 0, true);

    let buf = render_to_test_backend(&app);
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
    let rows = render::build_rows(view, &app);
    let placeholder = rows
        .first()
        .and_then(|row| row.iter().find(|c| c.ch == '\u{2407}'))
        .expect("placeholder cell present in row 0");
    assert_eq!(
        placeholder.buf_offset, 1,
        "the BEL is the 2nd byte (offset 1) of \"a\\x07b\""
    );
}

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
