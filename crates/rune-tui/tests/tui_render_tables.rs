//! The DISPLAY-space table border synthesis
//! (`DisplaySnapshot::expand_tables`) reaching the real terminal render
//! through the full `App` pipeline, plus the `Document::scroll_to_cursor`
//! wrap<->display conversion regression (the
//! one miss that scrolls every table document wrong by the number of
//! border rows above the cursor, and that no pre-existing test caught).
//!
//! Split out of `tests/tui_render.rs` rather than appended to it (that
//! file was already at 520 lines, over the 500-line budget, before
//! this work) — reuses its `testgrid`/`app_for`/`EDITOR_TOP_ROW`
//! conventions, duplicated locally per this crate's own established
//! per-test-file pattern (`tests/chrome.rs`, `tests/banner.rs` each keep
//! their own copy of the same small helpers rather than sharing one across
//! separate test binaries).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use ratatui::buffer::Buffer as RtBuffer;
use ratatui::style::Modifier;

use rune_core::coords::{DisplayRow, WrapRow};
use rune_fuzz::Session;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::testgrid;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

/// See `tests/tui_render.rs`'s own copy of this constant for the chrome-row
/// accounting this pins against.
const EDITOR_TOP_ROW: u16 = 2;

/// See `tests/tui_render.rs`'s own copy: the center `Block::bordered()`
/// puts a `│` at backend column 0, so the editor's own column 0 is backend
/// column 1.
const EDITOR_LEFT_COL: usize = 1;

const RIGHT: KeyInput = KeyInput {
    code: KeyCode::Right,
    mods: Mods::NONE,
};

/// Walks the active document's caret from `Session::open`'s own byte 0 up to
/// `offset`, one `Right` press per grapheme step — the real navigation path,
/// never a `CursorSet::new` poke.
fn place_caret(session: &mut Session, offset: usize) {
    let target = offset.min(session.app().active_doc().buffer.content().len());
    let mut guard = 0usize;
    while session.app().active_doc().cursors.primary().position.get() < target {
        session.key(RIGHT);
        guard += 1;
        assert!(
            guard <= target + 8,
            "caret placement stalled before reaching offset {target}"
        );
    }
}

fn app_for(content: &str, cursor_offset: usize, focused: bool) -> Session {
    let mut session = Session::open("/doc.md", content);
    session.resize(WIDTH, HEIGHT);
    place_caret(&mut session, cursor_offset);
    if !focused {
        use rune_tui::pane::Pane;
        use rune_tui::runtime::Effects;
        // Focus is gated on `LayoutMode` — show the column first so
        // `Explorer` is actually painted and the fixture keeps landing
        // focus off the Editor as intended. The focus change itself
        // doesn't go through `app::update`'s own post-dispatch sync, so
        // this `sync_view()`s explicitly rather than leaving conceal/caret
        // state stale for whatever reads it next.
        let mut effects = Effects::default();
        session.app_mut().splits.left.show();
        session
            .app_mut()
            .set_focus_pane(Pane::Explorer, &mut effects);
        session.app_mut().sync_view();
    }
    session
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

/// The synthesised top/bottom borders around a table with
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
    let session = app_for(TWO_BODY_ROW_TABLE, cursor, true);

    let top = testgrid::row(session.app(), EDITOR_TOP_ROW, WIDTH, HEIGHT);
    let top_editor_text: String = top.chars().skip(EDITOR_LEFT_COL).collect();
    assert!(
        top_editor_text.starts_with('┌'),
        "expected the synthesised top border on the table's first editor row:\n{top:?}"
    );

    let bottom = testgrid::row(session.app(), EDITOR_TOP_ROW + 6, WIDTH, HEIGHT);
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

/// A regression: `Document::scroll_to_cursor` must convert the
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
    let session = app_for(&content, cursor, true);
    let app = session.app();

    let view = app.active_doc().view.as_ref().expect("synced view");
    let buffer_point = app.active_doc().buffer.offset_to_line_col(cursor);
    let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
    let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
    let expected_display_row = view.display.wrap_to_display(WrapRow(wrap_point.row));

    let scroll_row = app.active_doc().viewport.scroll_row;
    assert!(
        scroll_row > DisplayRow(0),
        "fixture must be long enough to force real scrolling, or this test is vacuous"
    );

    let buf = testgrid::draw(app, WIDTH, HEIGHT);
    let actual_backend_row =
        caret_row(&buf, HEIGHT, WIDTH).expect("caret must be visible somewhere on screen");
    let actual_display_row = scroll_row + (actual_backend_row - EDITOR_TOP_ROW) as usize;

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

#[test]
fn caret_inside_a_ragged_rows_dropped_cells_stays_hidden_while_unfocused() {
    let cursor = RAGGED_ROW_TABLE
        .find(" a | b |\n")
        .map(|i| i + " a | b |".len())
        .expect("fixture has the ragged row's dropped tail");
    let session = app_for(RAGGED_ROW_TABLE, cursor, false);
    let app = session.app();

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = rune_tui::render::build_rows(app, app.active_doc(), Some(app.active), view);
    let meta = rune_tui::row_meta::row_meta(view, app);

    let widths: Vec<usize> = rows
        .iter()
        .map(|row| row.iter().map(|c| c.width as usize).sum())
        .collect();

    let boxed_widths: Vec<usize> = meta
        .iter()
        .zip(&widths)
        .filter(|(m, _)| m.boxed)
        .map(|(_, &w)| w)
        .collect();
    assert!(
        !boxed_widths.is_empty(),
        "the table's header and body rows must still render inside a box"
    );
    let first = boxed_widths[0];
    assert!(
        boxed_widths.iter().all(|&w| w == first),
        "every boxed row in the table's own box must share the same summed \
         cell width, got {boxed_widths:?}"
    );

    let ragged_row_meta = rows
        .iter()
        .zip(&meta)
        .find(|(row, _)| {
            row.iter()
                .map(|c| c.text.as_str())
                .collect::<String>()
                .contains("Bob")
        })
        .map(|(_, m)| m)
        .expect("the ragged row must still render its raw source on screen");
    assert!(
        !ragged_row_meta.boxed && ragged_row_meta.table_group.is_none(),
        "a truncated row must render outside the table's own box, got {ragged_row_meta:?}"
    );

    let buf = testgrid::draw(app, WIDTH, HEIGHT);
    assert_eq!(
        caret_row(&buf, HEIGHT, WIDTH),
        None,
        "an unfocused editor must show no caret at all, per Document::has_insertion_point"
    );
}
