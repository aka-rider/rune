//! WP3 done-when: the DISPLAY-space table border synthesis
//! (`DisplaySnapshot::expand_tables`) reaching the real terminal render
//! through the full `App` pipeline, plus the `Document::scroll_to_cursor`
//! wrap<->display conversion regression (plan WP3.S5's first bullet: the
//! one miss that scrolls every table document wrong by the number of
//! border rows above the cursor, and that no pre-existing test caught).
//!
//! Split out of `tests/tui_render.rs` rather than appended to it (§1.6:
//! that file was already at 520 lines, over the 500-line budget, before
//! this work) — reuses its `testgrid`/`app_for`/`EDITOR_TOP_ROW`
//! conventions, duplicated locally per this crate's own established
//! per-test-file pattern (`tests/chrome.rs`, `tests/banner.rs` each keep
//! their own copy of the same small helpers rather than sharing one across
//! separate test binaries).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use ratatui::buffer::Buffer as RtBuffer;
use ratatui::style::Modifier;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::App;
use rune_tui::pane::Pane;
use rune_tui::testgrid;
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

/// See `tests/tui_render.rs`'s own copy of this constant for the chrome-row
/// accounting this pins against.
const EDITOR_TOP_ROW: u16 = 2;

/// See `tests/tui_render.rs`'s own copy: the center `Block::bordered()`
/// puts a `│` at backend column 0, so the editor's own column 0 is backend
/// column 1.
const EDITOR_LEFT_COL: usize = 1;

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

/// The backend row carrying the cursor's reverse-video overlay, searched
/// across the WHOLE frame (unlike `tests/tui_render.rs`'s `caret_column`,
/// which takes the row as a given) — this file doesn't know in advance
/// which row a caret sitting below a multi-row table lands on.
fn caret_row(buf: &RtBuffer, height: u16, width: u16) -> Option<u16> {
    (0..height).find(|&y| {
        (0..width).any(|x| {
            buf.cell((x, y))
                .is_some_and(|c| c.modifier.contains(Modifier::REVERSED))
        })
    })
}

/// A table with TWO body rows (no source line of its own separates them),
/// so `DisplaySnapshot::expand_tables` must synthesise a `├┼┤` between
/// them, on top of the outer `┌┬┐`/`└┴┘` — the 4-source-line table this
/// file's other test also pins its own row-count expectation against
/// (`crates/rune-md/src/snapshot.rs`'s own unit tests). Cursor sits in the
/// trailing "tail" paragraph, well outside the table's own lines, so the
/// table stays `Rendered` rather than revealing its raw source.
const TWO_BODY_ROW_TABLE: &str =
    "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |\n\ntail\n";

/// WP3 Done-when: the synthesised top/bottom borders around a table with
/// an inter-row border between its two body rows actually reach the
/// terminal grid at the rows the geometry predicts — a top border on the
/// table's very first editor row, and (7 rows later: top, header,
/// separator, Alice, the synthesised inter-row border, Bob, bottom) a
/// bottom border.
#[test]
fn table_borders_render_at_the_predicted_display_rows() {
    let cursor = TWO_BODY_ROW_TABLE
        .find("tail")
        .expect("fixture has a tail paragraph");
    let app = app_for(TWO_BODY_ROW_TABLE, cursor, true);

    let top = testgrid::row(&app, EDITOR_TOP_ROW, WIDTH, HEIGHT);
    let top_editor_text: String = top.chars().skip(EDITOR_LEFT_COL).collect();
    assert!(
        top_editor_text.starts_with('┌'),
        "expected the synthesised top border on the table's first editor row:\n{top:?}"
    );

    let bottom = testgrid::row(&app, EDITOR_TOP_ROW + 6, WIDTH, HEIGHT);
    let bottom_editor_text: String = bottom.chars().skip(EDITOR_LEFT_COL).collect();
    assert!(
        bottom_editor_text.starts_with('└'),
        "expected the synthesised bottom border 6 rows below the top border:\n{bottom:?}"
    );
}

/// A table with two body rows (three synthesised border rows), followed by
/// enough filler lines that the document does NOT fit in a normal-sized
/// viewport — cursor placed on the very LAST line forces `scroll_to_cursor`
/// to actually move `scroll_row` off 0. A short fixture (the whole document
/// fitting on screen) would leave `scroll_row` at 0 regardless of whether
/// the wrap<->display conversion is present, making the regression
/// invisible — this is deliberately long enough that it isn't.
fn table_then_many_lines(n: usize) -> String {
    let mut s = TWO_BODY_ROW_TABLE.replace("tail\n", "");
    for i in 0..n {
        s.push_str(&format!("line {i}\n"));
    }
    s
}

/// The WP3.S5 regression: `Document::scroll_to_cursor` must convert the
/// cursor's WRAP row through `DisplaySnapshot::wrap_to_display` before
/// handing it to `Viewport::reconcile` (and convert the row `reconcile`
/// hands back the other way before snapping the cursor) — miss either
/// conversion and a document with a table above the cursor scrolls wrong
/// by the number of border rows the table synthesised (here, 3). The
/// caret's rendered row within the viewport, plus the settled
/// `scroll_row`, must recover the SAME absolute display row
/// `view.display.wrap_to_display` computes independently.
#[test]
fn caret_row_below_a_table_matches_wrap_to_display_of_its_wrap_row() {
    let content = table_then_many_lines(60);
    let cursor = content.len(); // the very last byte: last line, forces scrolling
    let app = app_for(&content, cursor, true);

    let view = app.active_doc().view.as_ref().expect("synced view");
    let buffer_point = app.active_doc().buffer.offset_to_line_col(cursor);
    let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
    let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
    let expected_display_row = view.display.wrap_to_display(wrap_point.row);

    let scroll_row = app.active_doc().viewport.scroll_row;
    assert!(
        scroll_row > 0,
        "fixture must be long enough to force real scrolling, or this test is vacuous"
    );

    let buf = testgrid::draw(&app, WIDTH, HEIGHT);
    let actual_backend_row =
        caret_row(&buf, HEIGHT, WIDTH).expect("caret must be visible somewhere on screen");
    let actual_display_row = (actual_backend_row - EDITOR_TOP_ROW) as usize + scroll_row;

    assert_eq!(
        actual_display_row, expected_display_row,
        "caret's absolute display row (on-screen row + scroll_row) must equal \
         view.display.wrap_to_display(wrap_row) (backend row {actual_backend_row}, \
         scroll_row {scroll_row}, editor top row {EDITOR_TOP_ROW})"
    );
}

/// A body row with MORE `|`-delimited cells than the table's own header
/// count (2): comrak's own table parser silently drops the extra cells
/// (Gotcha, `crates/rune-md/src/table/layout.rs`'s `col_widths` docs) —
/// they contribute no rendered content at all — but the raw SOURCE line
/// still carries all of them, so the row's raw byte length runs well past
/// the substituted box text's own length.
const RAGGED_ROW_TABLE: &str =
    "| Name | Age |\n| :--- | ---: |\n| Alice | 30 |\n| Bob | 25 | a | b |\n\ntail\n";

/// Regression for the `TABLE-ROW-WIDTH` fuzz catch (`crates/rune-fuzz/
/// proptest-regressions/human_session.txt`, seed `cc 5f23e392...`):
/// placing the caret inside a ragged row's DROPPED cells (bytes comrak's
/// table parser never turned into a real column, past every visible cell
/// the row's own box actually renders) used to fall through
/// `place_caret`'s "caret sits past the last visible char" branch, which
/// appended a synthetic one-cell-wide EOL cursor cell — making that ONE
/// row a cell wider than the rest of its table group. The fix clamps a
/// BOXED row's caret onto its own last cell instead of ever growing it.
/// Cursor sits at the very end of the `Bob` row's raw source line (deep in
/// the dropped `"| a | b |"` tail), the exact position the fuzz seed's
/// typing landed on.
///
/// Measures the same quantity the fuzzer's own `TABLE-ROW-WIDTH` invariant
/// does — each row's own `Cell::width` values, summed — via
/// `render::build_rows` directly, NOT the backend terminal grid: a
/// synthetic EOL cursor cell appended after a row's closing `│`/`┤` reads
/// as ordinary editor-background padding on the terminal grid (indistinct
/// from any other trailing space), so only the underlying cell count
/// actually catches the regression.
#[test]
fn caret_inside_a_ragged_rows_dropped_cells_never_widens_that_rows_box() {
    let cursor = RAGGED_ROW_TABLE
        .find(" a | b |\n")
        .map(|i| i + " a | b |".len())
        .expect("fixture has the ragged row's dropped tail");
    // `focused: false` forces the table's Decide-policy `RevealSm` to
    // `Rendered` regardless of the cursor sitting inside its own lines
    // (`DocMachine::sync_cursors`'s `RevealGrant::ForceRendered` root
    // grant) — the same reason `table_render.rs`'s own `synced` helper
    // always passes `focused: false` when checking Rendered content with
    // the cursor inside a table. Matches the fuzz seed's own end state:
    // that session's cursor sat inside the table too, yet the table
    // stayed boxed there because a dirty-close guard modal (from the
    // seed's `Ctrl+C`) made `App::sync_view` treat the editor as
    // unfocused for that step, the same net effect.
    let app = app_for(RAGGED_ROW_TABLE, cursor, false);

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = rune_tui::render::build_rows(view, &app);

    // Display rows 0..7: synthesised top border, header, separator,
    // Alice, the synthesised inter-row border between Alice and Bob (two
    // Body rows from different source lines), Bob (the ragged row),
    // synthesised bottom border — the table sits at the very start of the
    // document (`RAGGED_ROW_TABLE`), so these are the first 7 rows
    // `build_rows` returns (`scroll_row` is 0).
    let widths: Vec<usize> = rows
        .iter()
        .take(7)
        .map(|row| row.iter().map(|c| c.width as usize).sum())
        .collect();
    let first = widths.first().copied().unwrap_or(0);
    assert!(
        widths.iter().all(|&w| w == first),
        "every row in the table's own box must share the same summed cell \
         width, got {widths:?}"
    );

    let buf = testgrid::draw(&app, WIDTH, HEIGHT);
    assert!(
        caret_row(&buf, HEIGHT, WIDTH).is_some(),
        "the caret must still render somewhere, clamped rather than dropped"
    );
}
