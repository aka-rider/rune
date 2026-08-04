//! WP2/Go-parity done-when: headless render assertions on a `TestBackend`,
//! using the `Mem` vfs — a table's box-drawn layout and the focus/read-only
//! caret gate. TODO.md's §1.6 split of the original `tui_render.rs`:
//! conceal/styling/status-line/Cell-grid checks live in
//! `tui_render_basics.rs`, control-safe glyphs/tabs/graphemes in
//! `tui_render_text.rs`, and degenerate backend sizes/`blit`'s own
//! clipping in `tui_render_bounds.rs`. The runtime loop itself is NOT
//! exercised here (plan: "test the pure update/view paths headlessly; do
//! NOT spawn real terminals in tests") — every test drives `App`/
//! `render::draw` directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_render_common;

use rune_core::cursor::{Cursor, CursorSet};
use rune_tui::document::ReadOnly;
use rune_tui::testgrid;

use tui_render_common::{
    EDITOR_TOP_ROW, HEIGHT, WIDTH, app_for, caret_column, render_to_test_backend,
};

/// WP2 Done-when: a table's Grid layout reaches the real terminal render
/// through the full `App` pipeline, not just `rune-md`'s own `emit` unit
/// tests. Cursor sits in the trailing "tail" paragraph, well outside the
/// table's own lines, so the table stays `Rendered` (box-drawn) rather than
/// revealing its raw `| a | b |` source.
#[test]
fn table_renders_as_box_drawing_not_raw_pipes() {
    let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n\ntail\n";
    let cursor = content.find("tail").expect("fixture has a tail paragraph");
    let app = app_for(content, cursor, true);
    let row = testgrid::row(&app, EDITOR_TOP_ROW, WIDTH, HEIGHT);
    assert!(
        row.contains('│'),
        "expected the table's Grid border in the first editor row:\n{row:?}"
    );
    assert!(
        !row.contains('|'),
        "raw markdown pipe must not reach the rendered row:\n{row:?}"
    );
}

/// Go parity (`textedit/render.go`'s `m.focused && !m.readOnly` gate,
/// ported via `Document::has_insertion_point`): a caret must not render once the
/// editor pane loses focus. Both fixtures share the same content/cursor
/// offset, so the assertion can't pass vacuously by the caret simply
/// landing on a different row than the one checked.
#[test]
fn caret_not_visible_when_unfocused() {
    let content = "hello world\n";
    let offset = 3;

    let unfocused = app_for(content, offset, false);
    let buf = render_to_test_backend(&unfocused);
    assert_eq!(
        caret_column(&buf, EDITOR_TOP_ROW, WIDTH),
        None,
        "an unfocused editor must not paint a caret"
    );

    let focused = app_for(content, offset, true);
    let buf = render_to_test_backend(&focused);
    assert!(
        caret_column(&buf, EDITOR_TOP_ROW, WIDTH).is_some(),
        "the focused counterpart must still show a caret, or this test is vacuous"
    );
}

/// Go parity, selection half: `applyOverlays` gates the selection
/// background on the same `focused && !readOnly` predicate as the caret
/// (`textedit/render.go`), and `apply_cursor_overlays`'s `show_overlays`
/// early return covers both in one place — this pins the selection side of
/// that single gate.
#[test]
fn selection_not_highlighted_when_unfocused() {
    let content = "hello world\n";

    let mut unfocused = app_for(content, 0, false);
    let id = unfocused.active;
    unfocused.doc_mut(id).unwrap().cursors = CursorSet::new_from(&[Cursor {
        position: 5,
        anchor: 0,
        desired_col: 0,
        id: 1,
    }]);
    unfocused.sync_view();
    let buf = render_to_test_backend(&unfocused);
    let selection_bg = unfocused.theme.chrome.selection_bg;
    let has_selection = (0..WIDTH).any(|x| {
        buf.cell((x, EDITOR_TOP_ROW))
            .is_some_and(|c| c.bg == selection_bg)
    });
    assert!(
        !has_selection,
        "an unfocused editor must not paint the selection background"
    );

    let mut focused = app_for(content, 0, true);
    let id = focused.active;
    focused.doc_mut(id).unwrap().cursors = CursorSet::new_from(&[Cursor {
        position: 5,
        anchor: 0,
        desired_col: 0,
        id: 1,
    }]);
    focused.sync_view();
    let buf = render_to_test_backend(&focused);
    let selection_bg = focused.theme.chrome.selection_bg;
    let has_selection = (0..WIDTH).any(|x| {
        buf.cell((x, EDITOR_TOP_ROW))
            .is_some_and(|c| c.bg == selection_bg)
    });
    assert!(
        has_selection,
        "the focused counterpart must still show the selection, or this test is vacuous"
    );
}

/// Go parity, the read-only half: the virtual Help document (and any other
/// read-only document) has no insertion point to point at, so it must show
/// no caret even while focused — `Document::has_insertion_point` folds
/// `is_read_only()` into the same gate as `focused`.
#[test]
fn caret_not_visible_on_a_read_only_document() {
    let content = "hello world\n";
    let offset = 3;
    let mut app = app_for(content, offset, true);
    let id = app.active;
    app.doc_mut(id).unwrap().read_only = ReadOnly::Always;
    app.sync_view();
    let buf = render_to_test_backend(&app);
    assert_eq!(
        caret_column(&buf, EDITOR_TOP_ROW, WIDTH),
        None,
        "a read-only document must not paint a caret"
    );
}
