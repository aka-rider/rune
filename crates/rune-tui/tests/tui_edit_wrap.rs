#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_edit_common;

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::coords::{BufferOffset, VisualCol};
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

use tui_edit_common::{app_for, full_text, key, press, render_to_test_backend};

#[test]
fn desired_col_survives_a_vertical_move_across_wrapped_rows() {
    let content = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ\n";
    let mut app = app_for(content, 0);
    app.active_doc_mut()
        .viewport
        .set_size(8, tui_edit_common::HEIGHT - 1);

    for _ in 0..5 {
        press(&mut app, KeyCode::Right, Mods::NONE);
    }
    let after_right = app.active_doc_mut().cursors.primary();
    assert_eq!(after_right.position, BufferOffset(5));
    assert_eq!(after_right.desired_col, VisualCol(5));

    press(&mut app, KeyCode::Down, Mods::NONE);
    let after_down = app.active_doc_mut().cursors.primary();
    assert_eq!(
        after_down.desired_col,
        VisualCol(5),
        "desired_col must be preserved across a vertical move"
    );

    let view = app.active_doc_mut().view();
    let bp = app
        .active_doc_mut()
        .buffer
        .offset_to_line_col(after_down.position.get());
    let sp = view.syntax.buffer_to_syntax(bp);
    let wp = view.wrap.syntax_to_wrap(sp);
    assert_eq!(
        view.wrap.visual_col(content, wp.row, wp.col),
        5,
        "the caret must land on visual column 5 of the next wrap row"
    );
}

const INLINE_CODE_PARAGRAPH: &str = "Explore many databases in one search with our helpful `Explore` command to find results quickly across all your files today.\n";
const INLINE_CODE_WIDTH: u16 = 37;

fn wrap_row_of(app: &mut App, pos: usize) -> usize {
    let view = app.active_doc_mut().view();
    let bp = app.active_doc_mut().buffer.offset_to_line_col(pos);
    let sp = view.syntax.buffer_to_syntax(bp);
    view.wrap.syntax_to_wrap(sp).row
}

#[test]
fn down_from_inside_an_inline_code_span_advances_one_wrap_row() {
    let backtick = INLINE_CODE_PARAGRAPH.find('`').unwrap();
    let between_the_backticks = backtick + 3;
    let mut app = app_for(INLINE_CODE_PARAGRAPH, between_the_backticks);
    app.active_doc_mut()
        .viewport
        .set_size(INLINE_CODE_WIDTH, tui_edit_common::HEIGHT - 1);

    let origin_row = wrap_row_of(&mut app, between_the_backticks);
    assert_eq!(
        origin_row, 1,
        "test fixture must place the span on the second wrap row"
    );

    press(&mut app, KeyCode::Down, Mods::NONE);

    let after = app.active_doc_mut().cursors.primary().position;
    let landed_row = wrap_row_of(&mut app, after.get());
    assert_eq!(
        landed_row,
        origin_row + 1,
        "Down from inside a concealed-on-move code span must advance one wrap row"
    );
}

#[test]
fn down_from_the_opening_backtick_of_an_inline_code_span_advances_one_wrap_row() {
    let backtick = INLINE_CODE_PARAGRAPH.find('`').unwrap();
    let mut app = app_for(INLINE_CODE_PARAGRAPH, backtick);
    app.active_doc_mut()
        .viewport
        .set_size(INLINE_CODE_WIDTH, tui_edit_common::HEIGHT - 1);

    let origin_row = wrap_row_of(&mut app, backtick);
    assert_eq!(
        origin_row, 1,
        "test fixture must place the span on the second wrap row"
    );
    let origin_col = app.active_doc_mut().cursors.primary().position;

    press(&mut app, KeyCode::Down, Mods::NONE);

    let after = app.active_doc_mut().cursors.primary().position;
    let landed_row = wrap_row_of(&mut app, after.get());
    assert_eq!(
        landed_row,
        origin_row + 1,
        "Down from the opening backtick must advance one wrap row, not stay on the origin row"
    );
    assert_ne!(
        after,
        origin_col + 1,
        "Down must not degrade into a same-row right shift"
    );
}

#[test]
fn line_up_from_below_an_inline_code_span_returns_to_the_origin_row() {
    let leading_line = "intro\n";
    let content = format!("{leading_line}{INLINE_CODE_PARAGRAPH}");
    let backtick = leading_line.len() + INLINE_CODE_PARAGRAPH.find('`').unwrap();
    let between_the_backticks = backtick + 3;
    let mut app = app_for(&content, between_the_backticks);
    app.active_doc_mut()
        .viewport
        .set_size(INLINE_CODE_WIDTH, tui_edit_common::HEIGHT - 1);

    let origin_row = wrap_row_of(&mut app, between_the_backticks);
    press(&mut app, KeyCode::Down, Mods::NONE);
    press(&mut app, KeyCode::Up, Mods::NONE);

    let after = app.active_doc_mut().cursors.primary().position;
    let landed_row = wrap_row_of(&mut app, after.get());
    assert_eq!(
        landed_row, origin_row,
        "Down then Up over an inline code span must return to the origin wrap row"
    );
}

#[test]
fn page_down_over_an_inline_code_span_advances_by_the_page_step() {
    let backtick = INLINE_CODE_PARAGRAPH.find('`').unwrap();
    let between_the_backticks = backtick + 3;
    let mut app = app_for(INLINE_CODE_PARAGRAPH, between_the_backticks);
    app.active_doc_mut().viewport.set_size(INLINE_CODE_WIDTH, 4);

    let origin_row = wrap_row_of(&mut app, between_the_backticks);
    press(&mut app, KeyCode::PageDown, Mods::NONE);

    let after = app.active_doc_mut().cursors.primary().position;
    let landed_row = wrap_row_of(&mut app, after.get());
    assert_eq!(
        landed_row,
        origin_row + 3,
        "page down (viewport height 4, page step 3) must advance the wrap row by the page step"
    );
}

#[test]
fn down_advances_both_cursors_when_one_sits_inside_an_inline_code_span() {
    let leading_line = "ab\n";
    let content = format!("{leading_line}{INLINE_CODE_PARAGRAPH}");
    let backtick = leading_line.len() + INLINE_CODE_PARAGRAPH.find('`').unwrap();
    let between_the_backticks = backtick + 3;
    let mut app = app_for(&content, between_the_backticks);
    app.active_doc_mut()
        .viewport
        .set_size(INLINE_CODE_WIDTH, tui_edit_common::HEIGHT - 1);

    let row_without_a_code_span = 1;
    app.active_doc_mut().cursors =
        CursorSet::new_from_positions(&[row_without_a_code_span, between_the_backticks]);

    let row_without_a_code_span_origin_row = wrap_row_of(&mut app, row_without_a_code_span);
    let between_the_backticks_origin_row = wrap_row_of(&mut app, between_the_backticks);

    press(&mut app, KeyCode::Down, Mods::NONE);

    let all = app.active_doc_mut().cursors.all().to_vec();
    assert_eq!(all.len(), 2, "both cursors must survive the move");
    let mut rows: Vec<usize> = all
        .iter()
        .map(|c| wrap_row_of(&mut app, c.position.get()))
        .collect();
    rows.sort_unstable();
    let mut expected = vec![
        row_without_a_code_span_origin_row + 1,
        between_the_backticks_origin_row + 1,
    ];
    expected.sort_unstable();
    assert_eq!(
        rows, expected,
        "both cursors must independently advance one wrap row"
    );
}

#[test]
fn shift_up_after_add_cursor_below_leaves_the_head_at_the_top() {
    let mut app = app_for("# Notes\n\ntail\n", 0);
    let alt_sup = Mods {
        shift: false,
        alt: true,
        ctrl: false,
        sup: true,
    };

    press(&mut app, KeyCode::Down, alt_sup);
    assert_eq!(
        app.active_doc_mut().cursors.all().len(),
        2,
        "AddCursorBelow must produce a second cursor"
    );

    let shift = Mods {
        shift: true,
        alt: false,
        ctrl: false,
        sup: false,
    };
    press(&mut app, KeyCode::Up, shift);

    let merged = app.active_doc_mut().cursors.primary();
    assert!(
        merged.has_selection(),
        "shift+Up must leave a selection behind"
    );
    assert_eq!(
        merged.position,
        merged.selection_start(),
        "the head of a selection made by pressing Up must sit at its top"
    );
}

#[test]
fn resize_then_key_in_the_same_batch_sees_the_post_resize_wrap() {
    let content = "0123456789\n";
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().focused = true;
    app.active_doc_mut().cursors = CursorSet::new(0);
    app.active_doc_mut()
        .viewport
        .set_size(80, tui_edit_common::HEIGHT - 1);
    app.sync_view();

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Resize(7, tui_edit_common::HEIGHT),
        &mut effects,
    );
    assert_eq!(app.active_doc_mut().viewport.width, 5);

    let mut effects2 = Effects::default();
    app::update(
        &mut app,
        Msg::Key(key(KeyCode::Down, Mods::NONE)),
        &mut effects2,
    );

    let after = app.active_doc_mut().cursors.primary();
    assert_eq!(
        after.position,
        BufferOffset(5),
        "Down must move within the narrow (width-5) wrap of the SAME logical \
         line, not skip to the trailing blank line a stale width-80 wrap \
         would have produced"
    );
}

#[test]
fn scroll_row_does_not_move_until_the_batch_settles() {
    let mut lines = String::new();
    for i in 0..100 {
        lines.push_str(&format!("line{i}\n"));
    }
    let mut app = app_for(&lines, 0);
    app.active_doc_mut()
        .viewport
        .set_size(tui_edit_common::WIDTH, 10);
    let scroll_before = app.active_doc_mut().viewport.scroll_row;

    let mut effects = Effects::default();
    for _ in 0..50 {
        app::update(
            &mut app,
            Msg::Key(key(KeyCode::Down, Mods::NONE)),
            &mut effects,
        );
    }
    assert_eq!(
        app.active_doc_mut().viewport.scroll_row,
        scroll_before,
        "scroll_row must not move until the batch settles"
    );

    app.sync_view();
    assert!(
        app.active_doc_mut().viewport.scroll_row > scroll_before,
        "scroll_row must follow the cursor once the batch settles"
    );
}

#[test]
fn moving_onto_a_heading_line_reveals_its_marker_moving_off_conceals_it() {
    let content = "plain text\n## Heading\nmore text\n";
    let mut app = app_for(content, 0);

    let buf0 = render_to_test_backend(&app);
    assert!(
        !full_text(&buf0).contains("## "),
        "heading concealed while the cursor is elsewhere"
    );

    press(&mut app, KeyCode::Down, Mods::NONE);
    let buf1 = render_to_test_backend(&app);
    assert!(
        full_text(&buf1).contains("## Heading"),
        "heading revealed while the cursor sits on its line"
    );

    press(&mut app, KeyCode::Down, Mods::NONE);
    let buf2 = render_to_test_backend(&app);
    assert!(
        !full_text(&buf2).contains("## "),
        "heading concealed again once the cursor moves off"
    );
}
