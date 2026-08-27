#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::pointer::ManualClock;
use crate::runtime::Msg;
use rune_core::buffer::Buffer;
use rune_core::coords::DisplayRow;
use rune_vfs::Mem;
use std::sync::Arc;

fn app_with(content: &str, width: u16, height: u16) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.clock = Arc::new(ManualClock::new());
    app.frame_width = width;
    app.frame_height = height + 1; // + footer row
    app.sync_view();
    app
}

/// `col`/`row` are relative to the EDITOR rect (what a gesture actually
/// hit-tests against) — translated here to the absolute frame
/// coordinates a real `MouseInput` carries, through the same
/// `layout::geometry` call `commands::mouse::handle` itself uses, so a
/// test can never silently click the border/title row instead of the
/// editor content.
fn editor_origin(app: &App) -> (u16, u16) {
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let editor = crate::layout::geometry(area, app).editor;
    (editor.x, editor.y)
}

#[derive(Clone, Copy, Default)]
struct Modifiers {
    shift: bool,
    alt: bool,
}

fn click(app: &mut App, kind: MouseKind, col: u16, row: u16) {
    click_modified(app, kind, col, row, Modifiers::default());
}

fn click_modified(app: &mut App, kind: MouseKind, col: u16, row: u16, modifiers: Modifiers) {
    let (ox, oy) = editor_origin(app);
    let mut effects = crate::runtime::Effects::default();
    crate::app::update(
        app,
        Msg::Mouse(MouseInput {
            kind,
            column: ox + col,
            row: oy + row,
            shift: modifiers.shift,
            alt: modifiers.alt,
            ctrl: false,
        }),
        &mut effects,
    );
}

#[test]
fn plain_click_positions_the_caret() {
    let mut app = app_with("hello world\n", 40, 10);
    click(&mut app, MouseKind::Down(MouseButton::Left), 6, 0);
    assert_eq!(app.active_doc().cursors.primary().position, 6);
    assert!(!app.active_doc().cursors.primary().has_selection());
}

#[test]
fn double_click_selects_the_word() {
    let mut app = app_with("hello world\n", 40, 10);
    click(&mut app, MouseKind::Down(MouseButton::Left), 2, 0);
    click(&mut app, MouseKind::Down(MouseButton::Left), 2, 0);
    let c = app.active_doc().cursors.primary();
    assert_eq!(c.selection_range(), (0, 5)); // "hello"
}

#[test]
fn triple_click_on_a_wrapped_line_selects_the_whole_logical_line() {
    // One long logical line, wrapped across several rows at width 10.
    let content = "aaaaaaaaaa bbbbbbbbbb cccccccccc\nsecond\n";
    let mut app = app_with(content, 10, 20);
    // The click lands on the SECOND wrapped row of the first logical
    // line (row 1), not its first — the gesture must still select the
    // whole logical line, every wrapped row included.
    click(&mut app, MouseKind::Down(MouseButton::Left), 2, 1);
    click(&mut app, MouseKind::Down(MouseButton::Left), 2, 1);
    click(&mut app, MouseKind::Down(MouseButton::Left), 2, 1);
    let c = app.active_doc().cursors.primary();
    let expected_end = "aaaaaaaaaa bbbbbbbbbb cccccccccc\n".len();
    assert_eq!(c.selection_range(), (0, expected_end));
}

#[test]
fn wheel_scrolls_three_rows_without_moving_the_cursor() {
    let content: String = (0..50).map(|i| format!("line {i}\n")).collect();
    let mut app = app_with(&content, 40, 10);
    let cursor_before = app.active_doc().cursors.primary().position;
    click(&mut app, MouseKind::ScrollDown, 0, 0);
    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(3));
    assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
}

#[test]
fn drag_after_a_plain_click_extends_the_selection() {
    let content: String = (0..5).map(|i| format!("line {i}\n")).collect();
    let mut app = app_with(&content, 40, 10);
    click(&mut app, MouseKind::Down(MouseButton::Left), 0, 0);
    click(&mut app, MouseKind::Drag(MouseButton::Left), 4, 2);
    let c = app.active_doc().cursors.primary();
    assert!(c.has_selection());
    assert_eq!(c.selection_start(), 0);
}

#[test]
fn alt_click_adds_a_cursor_without_disturbing_the_first() {
    let mut app = app_with("hello world\n", 40, 10);
    click(&mut app, MouseKind::Down(MouseButton::Left), 0, 0);
    click_modified(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        6,
        0,
        Modifiers {
            alt: true,
            ..Modifiers::default()
        },
    );
    assert!(app.active_doc().cursors.is_multi());
}

#[test]
fn shift_click_extends_the_selection() {
    let mut app = app_with("hello world\n", 40, 10);
    click(&mut app, MouseKind::Down(MouseButton::Left), 0, 0);
    click_modified(
        &mut app,
        MouseKind::Down(MouseButton::Left),
        5,
        0,
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
    );
    let c = app.active_doc().cursors.primary();
    assert_eq!(c.selection_range(), (0, 5));
}

#[test]
fn click_outside_the_editor_rect_is_ignored() {
    let mut app = app_with("hello\n", 40, 10);
    let cursor_before = app.active_doc().cursors.primary().position;
    // Row far below the editor's visible area.
    click(&mut app, MouseKind::Down(MouseButton::Left), 0, 200);
    assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
}

/// A click on a synthesised table border row must be a
/// complete no-op — never move the caret to some nearby
/// offset. The table sits at the very top of the document, so editor
/// row 0 is its synthesised `┌┬┐` top border (`DisplaySnapshot::
/// expand_tables`), with no wrap row of its own to click into. The
/// cursor is placed on the trailing "tail" paragraph BEFORE the
/// initial `sync_view` (not via a click, per `app_with`'s own docs:
/// `doc.view` is cached once per batch, so a click's hit-test always
/// sees the reveal state as of that initial sync) — otherwise the
/// default cursor at buffer offset 0 sits ON the table's own line,
/// which keeps it `Revealed` (raw text, no borders at all) and the
/// premise of this test never holds.
#[test]
fn click_on_a_synthetic_table_border_row_does_not_move_the_cursor() {
    let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n\ntail\n";
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.clock = Arc::new(ManualClock::new());
    app.frame_width = 40;
    app.frame_height = 21; // + footer row
    let cursor_offset = content.find("tail").expect("fixture has a tail paragraph");
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(cursor_offset);
    app.sync_view();
    let cursor_before = app.active_doc().cursors.primary().position;

    click(&mut app, MouseKind::Down(MouseButton::Left), 2, 0);

    assert_eq!(
        app.active_doc().cursors.primary().position,
        cursor_before,
        "a click on the synthesised top border must not move the caret"
    );
    assert!(!app.active_doc().cursors.primary().has_selection());
}

/// `Geometry::pane_at` deliberately classifies every `diff_left` column as
/// `Pane::Editor` too, so a latched text drag that wanders from the editor
/// into the diff-left pane must be stopped by `handle_left_drag`'s own
/// containment check against the editor's rect, not by `pane_at`. Before
/// that fix this reached `input.column - editor.x` with `input.column` left
/// of `editor.x`, an unchecked `u16` subtraction that panics in debug and
/// wraps to a garbage offset in release.
#[test]
fn drag_that_wanders_into_the_diff_left_pane_is_a_no_op() {
    let content: String = (0..5).map(|i| format!("line {i}\n")).collect();
    let mut app = app_with(&content, 100, 20);
    let right = app.active;
    crate::diff_view::install_text(
        &mut app,
        right,
        "left text\n".to_string(),
        "left.md".to_string(),
    );
    app.sync_view();

    click(&mut app, MouseKind::Down(MouseButton::Left), 5, 0);
    let cursor_before = app.active_doc().cursors.primary();

    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let geo = crate::layout::geometry(area, &app);
    let diff_left = geo
        .diff_left
        .expect("diff-left pane must be laid out at this width");
    assert_eq!(
        geo.pane_at(diff_left.x, diff_left.y),
        Some(Pane::Editor),
        "test setup: pane_at must still classify diff_left as Editor"
    );

    let mut effects = crate::runtime::Effects::default();
    crate::app::update(
        &mut app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::Drag(MouseButton::Left),
            column: diff_left.x,
            row: diff_left.y,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );

    let cursor_after = app.active_doc().cursors.primary();
    assert_eq!(
        cursor_after, cursor_before,
        "a drag into diff_left must not move or resize the selection"
    );
}

/// Finding 6: the shared click-count -> cursor shape, document-agnostic.
#[test]
fn place_click_cursor_and_extend_drag_cursor_are_document_agnostic() {
    let mut doc = crate::document::Document::new(Buffer::new("hello world\n"));
    assert!(place_click_cursor(&mut doc, 6, 6, 1));
    assert_eq!(doc.cursors.primary().position, 6);
    assert!(!place_click_cursor(&mut doc, 6, 6, 2));
    assert_eq!(doc.cursors.primary().selection_range(), (6, 11));
    assert!(!place_click_cursor(&mut doc, 6, 6, 3));
    assert_eq!(doc.cursors.primary().selection_range(), (0, 12));
    extend_drag_cursor(&mut doc, 0, 5, 5);
    let c = doc.cursors.primary();
    assert_eq!((c.anchor, c.position), (0, 5));
}
