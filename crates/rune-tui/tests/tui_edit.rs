//! WP6/WP7 done-when: movement, selection, editing, and undo/redo driven
//! through the real `app::update` (headless — `TestBackend` + `Mem` vfs
//! only, no wall-clock sleeps, no real terminal — mirrors `tests/tui_render.rs`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use ratatui::buffer::Buffer as RtBuffer;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::testgrid;
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn app_for(content: &str, cursor_offset: usize) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().focused = true;
    app.active_doc_mut().cursors = CursorSet::new(cursor_offset.min(content.len()));
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn key(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

/// Sends one `Msg::Key` through the real `update`, then resyncs `app.view`
/// (what the runtime does once per whole message batch — see
/// `runtime::run`) so render/scroll assertions see the settled state.
fn press(app: &mut App, code: KeyCode, mods: Mods) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(key(code, mods)), &mut effects);
    app.sync_view();
}

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};
const ALT: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: false,
};
const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};
const SUP_SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: true,
};

fn render_to_test_backend(app: &App) -> RtBuffer {
    testgrid::draw(app, WIDTH, HEIGHT)
}

fn full_text(buf: &RtBuffer) -> String {
    let mut s = String::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if let Some(cell) = buf.cell((x, y)) {
                s.push_str(cell.symbol());
            }
        }
        s.push('\n');
    }
    s
}

// ---- Movement matrix (WP6) ----

#[test]
fn char_right_then_left_returns_to_start() {
    let mut app = app_for("hello", 0);
    press(&mut app, KeyCode::Right, Mods::NONE);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 1);
    press(&mut app, KeyCode::Left, Mods::NONE);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 0);
}

#[test]
fn char_left_at_buffer_start_does_not_go_negative() {
    let mut app = app_for("hello", 0);
    press(&mut app, KeyCode::Left, Mods::NONE);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 0);
}

#[test]
fn word_right_left_navigate_word_boundaries() {
    let mut app = app_for("hello world", 0);
    press(&mut app, KeyCode::Right, ALT);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 5);
    press(&mut app, KeyCode::Left, ALT);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 0);
}

#[test]
fn home_end_move_to_line_boundaries() {
    let mut app = app_for("hello\nworld", 8); // caret inside "world"
    press(&mut app, KeyCode::Home, Mods::NONE);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 6);
    press(&mut app, KeyCode::End, Mods::NONE);
    assert_eq!(app.active_doc_mut().cursors.primary().position, 11);
}

#[test]
fn shift_right_extends_a_selection_plain_right_collapses_it() {
    let mut app = app_for("hello", 0);
    press(&mut app, KeyCode::Right, SHIFT);
    let c = app.active_doc_mut().cursors.primary();
    assert_eq!((c.anchor, c.position), (0, 1));
    assert!(c.has_selection());

    press(&mut app, KeyCode::Right, Mods::NONE);
    let c = app.active_doc_mut().cursors.primary();
    assert!(!c.has_selection(), "a plain move consumes the selection");
}

#[test]
fn select_all_selects_the_whole_buffer() {
    let mut app = app_for("hello world", 3);
    press(&mut app, KeyCode::Char('a'), SUP);
    let c = app.active_doc_mut().cursors.primary();
    assert_eq!((c.anchor, c.position), (0, 11));
}

#[test]
fn escape_collapses_a_selection_to_the_caret() {
    let mut app = app_for("hello", 0);
    press(&mut app, KeyCode::Right, SHIFT);
    assert!(app.active_doc_mut().cursors.primary().has_selection());
    press(&mut app, KeyCode::Escape, Mods::NONE);
    let c = app.active_doc_mut().cursors.primary();
    assert!(!c.has_selection());
    assert_eq!(c.position, 1);
}

#[test]
fn page_down_then_page_up_returns_to_the_original_row() {
    let mut lines = String::new();
    for i in 0..200 {
        lines.push_str(&format!("line{i}\n"));
    }
    let mut app = app_for(&lines, 0);
    let before = app.active_doc_mut().cursors.primary();

    press(&mut app, KeyCode::PageDown, Mods::NONE);
    let after_down = app.active_doc_mut().cursors.primary();
    assert_ne!(
        after_down.position, before.position,
        "page down must move the caret"
    );

    press(&mut app, KeyCode::PageUp, Mods::NONE);
    let after_up = app.active_doc_mut().cursors.primary();
    assert_eq!(
        after_up.position, before.position,
        "page up must return to the original row"
    );
}

/// Desired-col preservation across wrapped VISUAL rows within one long,
/// space-free logical line (plan: "desired-col preservation across wrap
/// rows"). Established by moving right to a known byte/visual column, then
/// verified to survive a vertical move to a different-length wrap row.
#[test]
fn desired_col_survives_a_vertical_move_across_wrapped_rows() {
    let content = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ\n"; // no spaces: force-break wrap
    let mut app = app_for(content, 0);
    app.active_doc_mut().viewport.set_size(8, HEIGHT - 1);

    for _ in 0..5 {
        press(&mut app, KeyCode::Right, Mods::NONE);
    }
    let after_right = app.active_doc_mut().cursors.primary();
    assert_eq!(after_right.position, 5);
    assert_eq!(after_right.desired_col, 5);

    press(&mut app, KeyCode::Down, Mods::NONE);
    let after_down = app.active_doc_mut().cursors.primary();
    assert_eq!(
        after_down.desired_col, 5,
        "desired_col must be preserved across a vertical move"
    );

    let view = app.active_doc_mut().view();
    let bp = app
        .active_doc_mut()
        .buffer
        .offset_to_line_col(after_down.position);
    let sp = view.syntax.buffer_to_syntax(bp);
    let wp = view.wrap.syntax_to_wrap(sp);
    assert_eq!(
        view.wrap.visual_col(content, wp.row, wp.col),
        5,
        "the caret must land on visual column 5 of the next wrap row"
    );
}

/// Regression for the plan's carried-forward review constraint: a `Key`
/// handled right after a `Resize` in the SAME message batch must see the
/// post-resize wrap, not a stale `app.view` (only refreshed once per
/// batch by the runtime — see `runtime::run`). `nav` handlers call
/// `Editor::sync()` fresh instead of reading the cache, so this must pass
/// even though `app.sync_view()` is deliberately NOT called between the
/// two `update` calls below (simulating same-batch delivery).
#[test]
fn resize_then_key_in_the_same_batch_sees_the_post_resize_wrap() {
    let content = "0123456789\n"; // one space-free 10-char line
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().focused = true;
    app.active_doc_mut().cursors = CursorSet::new(0);
    app.active_doc_mut().viewport.set_size(80, HEIGHT - 1); // wide: the whole line is one row
    app.sync_view();

    // Resizing the FRAME to 7 columns (not 5): the center pane's border
    // (plan WP4) eats 2 of those for its own left/right border cells,
    // leaving the editor's wrap width at exactly 5 — the width this test
    // actually wants to drive the "Down wraps at column 5" behavior below.
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Resize(7, HEIGHT), &mut effects);
    assert_eq!(app.active_doc_mut().viewport.width, 5);

    // No `app.sync_view()` here: this Key must be handled within the same
    // logical batch as the Resize above, purely off `Editor::sync()`.
    let mut effects2 = Effects::default();
    app::update(
        &mut app,
        Msg::Key(key(KeyCode::Down, Mods::NONE)),
        &mut effects2,
    );

    let after = app.active_doc_mut().cursors.primary();
    assert_eq!(
        after.position, 5,
        "Down must move within the narrow (width-5) wrap of the SAME logical \
         line, not skip to the trailing blank line a stale width-80 wrap \
         would have produced"
    );
}

/// Regression for F4: `viewport.scroll_row` has exactly one writer
/// (`Editor::scroll_to_cursor`, invoked once per settled batch via
/// `Editor::sync`/`App::sync_view`) — a nav command's internal
/// `Editor::view()` call must NEVER move it mid-motion.
#[test]
fn scroll_row_does_not_move_until_the_batch_settles() {
    let mut lines = String::new();
    for i in 0..100 {
        lines.push_str(&format!("line{i}\n"));
    }
    let mut app = app_for(&lines, 0);
    app.active_doc_mut().viewport.set_size(WIDTH, 10);
    let scroll_before = app.active_doc_mut().viewport.scroll_row;

    // Bypass the `press` helper (which calls `sync_view()` after every
    // key) to observe the PRE-settle state directly.
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

    // Settling the batch (what the runtime does once per whole message
    // batch) scrolls in one shot to follow the final cursor.
    app.sync_view();
    assert!(
        app.active_doc_mut().viewport.scroll_row > scroll_before,
        "scroll_row must follow the cursor once the batch settles"
    );
}

// ---- Reveal follows the cursor (WP6) ----

#[test]
fn moving_onto_a_heading_line_reveals_its_marker_moving_off_conceals_it() {
    let content = "plain text\n## Heading\nmore text\n";
    let mut app = app_for(content, 0); // caret on line 0

    let buf0 = render_to_test_backend(&app);
    assert!(
        !full_text(&buf0).contains("## "),
        "heading concealed while the cursor is elsewhere"
    );

    press(&mut app, KeyCode::Down, Mods::NONE); // -> line 1 (the heading)
    let buf1 = render_to_test_backend(&app);
    assert!(
        full_text(&buf1).contains("## Heading"),
        "heading revealed while the cursor sits on its line"
    );

    press(&mut app, KeyCode::Down, Mods::NONE); // -> line 2, off the heading
    let buf2 = render_to_test_backend(&app);
    assert!(
        !full_text(&buf2).contains("## "),
        "heading concealed again once the cursor moves off"
    );
}

// ---- Editing (WP7) ----

#[test]
fn typing_inserts_characters_in_order_and_moves_the_caret() {
    let mut app = app_for("", 0);
    for ch in "hi!".chars() {
        press(&mut app, KeyCode::Char(ch), Mods::NONE);
    }
    assert_eq!(app.active_doc_mut().buffer.content(), "hi!");
    assert_eq!(app.active_doc_mut().cursors.primary().position, 3);
}

#[test]
fn typing_marks_the_buffer_dirty() {
    let mut app = app_for("hi", 0);
    assert!(!app.is_dirty());
    press(&mut app, KeyCode::Char('!'), Mods::NONE);
    assert!(app.is_dirty());
}

#[test]
fn typing_over_a_selection_replaces_it() {
    let mut app = app_for("hello world", 0);
    for _ in 0..5 {
        press(&mut app, KeyCode::Right, SHIFT); // select "hello"
    }
    assert!(app.active_doc_mut().cursors.primary().has_selection());

    press(&mut app, KeyCode::Char('X'), Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "X world");
    let c = app.active_doc_mut().cursors.primary();
    assert_eq!(c.position, 1);
    assert!(!c.has_selection());
}

#[test]
fn backspace_key_removes_the_char_to_the_left() {
    let mut app = app_for("abc", 1);
    press(&mut app, KeyCode::Backspace, Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "bc");
    assert_eq!(app.active_doc_mut().cursors.primary().position, 0);
}

#[test]
fn delete_key_removes_the_char_to_the_right() {
    let mut app = app_for("abc", 0);
    press(&mut app, KeyCode::Delete, Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "bc");
    assert_eq!(app.active_doc_mut().cursors.primary().position, 0);
}

#[test]
fn enter_inserts_a_newline_preserving_indentation() {
    let mut app = app_for("  indented", 10);
    press(&mut app, KeyCode::Enter, Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "  indented\n  ");
}

#[test]
fn tab_indents_the_current_line_shift_tab_outdents_it() {
    let mut app = app_for("hello", 2);
    press(&mut app, KeyCode::Tab, Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "\thello");

    press(&mut app, KeyCode::Tab, SHIFT);
    assert_eq!(app.active_doc_mut().buffer.content(), "hello");
}

#[test]
fn tab_indents_every_line_of_a_shift_selected_block_and_keeps_the_selection() {
    let mut app = app_for("one\ntwo\nthree", 0);
    press(&mut app, KeyCode::Down, SHIFT);
    press(&mut app, KeyCode::Down, SHIFT);
    press(&mut app, KeyCode::End, SHIFT);
    press(&mut app, KeyCode::Tab, Mods::NONE);
    assert_eq!(
        app.active_doc_mut().buffer.content(),
        "\tone\n\ttwo\n\tthree"
    );
    assert!(app.active_doc_mut().cursors.primary().has_selection());

    press(&mut app, KeyCode::Tab, SHIFT);
    assert_eq!(app.active_doc_mut().buffer.content(), "one\ntwo\nthree");
}

// ---- Undo/redo (WP7) ----

#[test]
fn undo_restores_byte_exact_content_and_redo_reapplies_it() {
    let mut app = app_for("hello", 5);
    press(&mut app, KeyCode::Char('!'), Mods::NONE);
    assert_eq!(app.active_doc_mut().buffer.content(), "hello!");

    press(&mut app, KeyCode::Char('z'), SUP); // Undo
    assert_eq!(app.active_doc_mut().buffer.content(), "hello");

    press(&mut app, KeyCode::Char('z'), SUP_SHIFT); // Redo
    assert_eq!(app.active_doc_mut().buffer.content(), "hello!");
}

#[test]
fn undo_redo_never_split_a_cjk_or_emoji_char() {
    let mut app = app_for("你好", "你".len());
    let original = app.active_doc_mut().buffer.content().to_string();

    press(&mut app, KeyCode::Char('\u{1f389}'), Mods::NONE); // insert 🎉 between 你 and 好
    assert_eq!(app.active_doc_mut().buffer.content(), "你\u{1f389}好");

    press(&mut app, KeyCode::Char('z'), SUP); // Undo
    assert_eq!(
        app.active_doc_mut().buffer.content(),
        original,
        "undo must restore the original content byte-exact, including CJK"
    );

    press(&mut app, KeyCode::Char('z'), SUP_SHIFT); // Redo
    assert_eq!(app.active_doc_mut().buffer.content(), "你\u{1f389}好");
}

#[test]
fn undo_redo_restore_the_recorded_cursor_position() {
    let mut app = app_for("hello", 2);
    press(&mut app, KeyCode::Char('X'), Mods::NONE); // insert at 2 -> caret at 3
    assert_eq!(app.active_doc_mut().cursors.primary().position, 3);

    press(&mut app, KeyCode::Char('z'), SUP); // Undo
    assert_eq!(app.active_doc_mut().buffer.content(), "hello");
    assert_eq!(
        app.active_doc_mut().cursors.primary().position,
        2,
        "undo must restore the pre-edit cursor position"
    );

    press(&mut app, KeyCode::Char('z'), SUP_SHIFT); // Redo
    assert_eq!(
        app.active_doc_mut().cursors.primary().position,
        3,
        "redo must restore the post-edit cursor position"
    );
}

#[test]
fn undo_with_an_empty_journal_is_a_no_op() {
    let mut app = app_for("hello", 0);
    press(&mut app, KeyCode::Char('z'), SUP);
    assert_eq!(app.active_doc_mut().buffer.content(), "hello");
}

/// Moving straight onto a concealing heading with the caret at visual
/// column 0 must land BEFORE its `# ` marker (byte 0 of that line), not
/// after it — the marker is the leftmost thing on screen once the line
/// reveals, so a fresh caret placed there directly (no navigation at all)
/// sits at byte 0 too; this is that same landing spot reached via `Down`.
#[test]
fn moving_down_onto_a_heading_lands_before_its_marker() {
    let content = "plain\n# Heading\ntext\n";
    let mut app = app_for(content, 0);
    press(&mut app, KeyCode::Down, Mods::NONE);
    assert_eq!(
        app.active_doc_mut().cursors.primary().position,
        "plain\n".len()
    );
}

/// Down then Up back onto a concealing heading must not change where
/// typing lands versus a caret that never left the line: the round trip
/// through the concealed line below must resettle the caret's column
/// against the heading's own (revealed) layout, not the stale layout of
/// wherever it detoured through.
#[test]
fn down_then_up_over_a_concealing_heading_lands_where_a_fresh_caret_would() {
    let content = "# Heading\ntext\n";

    let mut baseline = app_for(content, 0);
    press(&mut baseline, KeyCode::Char('X'), Mods::NONE);
    let baseline_content = baseline.active_doc_mut().buffer.content().to_string();

    let mut roundtrip = app_for(content, 0);
    press(&mut roundtrip, KeyCode::Down, Mods::NONE);
    press(&mut roundtrip, KeyCode::Up, Mods::NONE);
    assert_eq!(
        roundtrip.active_doc_mut().cursors.primary().position,
        0,
        "the round trip must resettle the caret back to byte 0, not inside the revealed text"
    );
    press(&mut roundtrip, KeyCode::Char('X'), Mods::NONE);
    let roundtrip_content = roundtrip.active_doc_mut().buffer.content().to_string();

    assert_eq!(
        roundtrip_content, baseline_content,
        "a Down/Up round trip over a concealing heading must not change where typing lands"
    );
}
