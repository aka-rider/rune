//! WP2 done-when: headless render assertions on a `TestBackend`,
//! using the `Mem` vfs — a table's box-drawn layout and the focus/read-only
//! caret gate. TODO.md's 500-line budget split of the original `tui_render.rs`:
//! conceal/styling/status-line/Cell-grid checks live in
//! `tui_render_basics.rs`, control-safe glyphs/tabs/graphemes in
//! `tui_render_text.rs`, and degenerate backend sizes/`blit`'s own
//! clipping in `tui_render_bounds.rs`. The runtime loop itself is NOT
//! exercised here (plan: "test the pure update/view paths headlessly; do
//! NOT spawn real terminals in tests") — every test drives `App`/
//! `render::draw` directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_render_common;

use rune_tui::document::ReadOnly;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::testgrid;

use tui_render_common::{
    EDITOR_TOP_ROW, HEIGHT, WIDTH, app_for, caret_column, render_to_test_backend,
};

const SHIFT_RIGHT: KeyInput = KeyInput {
    code: KeyCode::Right,
    mods: Mods {
        shift: true,
        ..Mods::NONE
    },
};

/// Extends the active document's selection from wherever the caret already
/// sits up to `target`, one `Shift+Right` press per grapheme step — the real
/// selection gesture, never a `CursorSet::new_from` poke.
fn extend_selection_to(session: &mut rune_fuzz::Session, target: usize) {
    let mut guard = 0usize;
    while session.app().active_doc().cursors.primary().position < target {
        session.key(SHIFT_RIGHT);
        guard += 1;
        assert!(
            guard <= target + 8,
            "selection extension stalled before reaching offset {target}"
        );
    }
}

/// WP2 Done-when: a table's Grid layout reaches the real terminal render
/// through the full `App` pipeline, not just `rune-md`'s own `emit` unit
/// tests. Cursor sits in the trailing "tail" paragraph, well outside the
/// table's own lines, so the table stays `Rendered` (box-drawn) rather than
/// revealing its raw `| a | b |` source.
#[test]
fn table_renders_as_box_drawing_not_raw_pipes() {
    let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n\ntail\n";
    let cursor = content.find("tail").expect("fixture has a tail paragraph");
    let session = app_for(content, cursor, true);
    let row = testgrid::row(session.app(), EDITOR_TOP_ROW, WIDTH, HEIGHT);
    assert!(
        row.contains('│'),
        "expected the table's Grid border in the first editor row:\n{row:?}"
    );
    assert!(
        !row.contains('|'),
        "raw markdown pipe must not reach the rendered row:\n{row:?}"
    );
}

/// A caret must not render once the editor pane loses focus
/// (`Document::has_insertion_point` gates it on `focused`). Both fixtures share the same content/cursor
/// offset, so the assertion can't pass vacuously by the caret simply
/// landing on a different row than the one checked.
#[test]
fn caret_not_visible_when_unfocused() {
    let content = "hello world\n";
    let offset = 3;

    let unfocused = app_for(content, offset, false);
    let buf = render_to_test_backend(unfocused.app());
    assert_eq!(
        caret_column(&buf, EDITOR_TOP_ROW, WIDTH),
        None,
        "an unfocused editor must not paint a caret"
    );

    let focused = app_for(content, offset, true);
    let buf = render_to_test_backend(focused.app());
    assert!(
        caret_column(&buf, EDITOR_TOP_ROW, WIDTH).is_some(),
        "the focused counterpart must still show a caret, or this test is vacuous"
    );
}

/// The selection half: `apply_cursor_overlays`'s `show_overlays` early
/// return gates the selection background on the same `focused && !read_only`
/// predicate as the caret, covering both in one place — this pins the
/// selection side of that single gate.
#[test]
fn selection_not_highlighted_when_unfocused() {
    let content = "hello world\n";

    // Built focused so the `Shift+Right` selection gesture actually reaches
    // the editor (keys route to whichever pane holds focus), then unfocused
    // afterward — `tui_render_common::unfocus` is the same focus-move
    // `app_for(..., false)` itself performs.
    let mut unfocused = app_for(content, 0, true);
    extend_selection_to(&mut unfocused, 5);
    tui_render_common::unfocus(&mut unfocused);
    let buf = render_to_test_backend(unfocused.app());
    let selection_bg = unfocused.app().theme.chrome.selection_bg;
    let has_selection = (0..WIDTH).any(|x| {
        buf.cell((x, EDITOR_TOP_ROW))
            .is_some_and(|c| c.bg == selection_bg)
    });
    assert!(
        !has_selection,
        "an unfocused editor must not paint the selection background"
    );

    let mut focused = app_for(content, 0, true);
    extend_selection_to(&mut focused, 5);
    let buf = render_to_test_backend(focused.app());
    let selection_bg = focused.app().theme.chrome.selection_bg;
    let has_selection = (0..WIDTH).any(|x| {
        buf.cell((x, EDITOR_TOP_ROW))
            .is_some_and(|c| c.bg == selection_bg)
    });
    assert!(
        has_selection,
        "the focused counterpart must still show the selection, or this test is vacuous"
    );
}

/// The read-only half: the virtual Help document (and any other
/// read-only document) has no insertion point to point at, so it must show
/// no caret even while focused — `Document::has_insertion_point` folds
/// `is_read_only()` into the same gate as `focused`.
#[test]
fn caret_not_visible_on_a_read_only_document() {
    let content = "hello world\n";
    let offset = 3;
    let mut session = app_for(content, offset, true);
    let id = session.app().active;
    session.app_mut().doc_mut(id).unwrap().read_only = ReadOnly::Always;
    session.app_mut().sync_view();
    let buf = render_to_test_backend(session.app());
    assert_eq!(
        caret_column(&buf, EDITOR_TOP_ROW, WIDTH),
        None,
        "a read-only document must not paint a caret"
    );
}
