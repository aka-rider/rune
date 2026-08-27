//! Jump-to-matching-bracket driven through the real `app::update` seam.
//! A sibling of `tui_edit.rs` rather than more rows inside it: that file is
//! already at the 500-line budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

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
const SUP_ALT: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: true,
};

fn app_for(content: &str, cursor_offset: usize) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().focused = true;
    app.active_doc_mut().cursors = CursorSet::new(cursor_offset.min(content.len()));
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn press(app: &mut App, code: KeyCode, mods: Mods) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(KeyInput { code, mods }), &mut effects);
    app.sync_view();
}

fn position(app: &App) -> usize {
    app.active_doc().cursors.primary().position
}

#[test]
fn sup_backslash_jumps_between_the_two_endpoints_of_the_pair_under_the_caret() {
    let content = "a (b [c] d) e";
    let open = content.find('(').unwrap();
    let close = content.find(')').unwrap();
    let mut app = app_for(content, open);

    press(&mut app, KeyCode::Char('\\'), SUP);
    assert_eq!(position(&app), close);

    press(&mut app, KeyCode::Char('\\'), SUP);
    assert_eq!(position(&app), open, "the jump must be its own inverse");
}

#[test]
fn sup_backslash_off_a_bracket_scans_the_rest_of_the_line_for_one() {
    let content = "a (b) c";
    let mut app = app_for(content, 0);

    press(&mut app, KeyCode::Char('\\'), SUP);
    assert_eq!(
        position(&app),
        content.find(')').unwrap(),
        "the vim-style line scan starts at the first bracket after the caret \
         and lands on that bracket's match"
    );
}

#[test]
fn sup_backslash_on_a_bracketless_line_leaves_the_caret_where_it_was() {
    let mut app = app_for("abc\n(d)", 1);

    press(&mut app, KeyCode::Char('\\'), SUP);
    assert_eq!(
        position(&app),
        1,
        "the line scan must stop at the newline, never wander onto the next line"
    );
}

#[test]
fn sup_shift_backslash_extends_the_selection_to_the_match() {
    let content = "(abc)";
    let mut app = app_for(content, 0);

    press(&mut app, KeyCode::Char('\\'), SUP_SHIFT);

    let c = app.active_doc().cursors.primary();
    assert_eq!(c.anchor, 0, "extending must leave the anchor put");
    assert_eq!(c.position, content.find(')').unwrap());
    assert!(c.has_selection());
}

#[test]
fn the_pipe_alternate_encoding_extends_exactly_like_sup_shift_backslash() {
    let content = "(abc)";
    let mut app = app_for(content, 0);

    press(&mut app, KeyCode::Char('|'), SUP_SHIFT);

    let c = app.active_doc().cursors.primary();
    assert_eq!(c.anchor, 0);
    assert_eq!(c.position, content.find(')').unwrap());
    assert!(c.has_selection());
}

#[test]
fn every_cursor_in_the_set_jumps_to_its_own_match() {
    let content = "(a)\n(b)";
    let mut app = app_for(content, 0);

    press(&mut app, KeyCode::Down, SUP_ALT);
    assert_eq!(
        app.active_doc().cursors.len(),
        2,
        "the fixture needs a real second cursor to prove per-cursor application"
    );

    press(&mut app, KeyCode::Char('\\'), SUP);

    let positions: Vec<usize> = app
        .active_doc()
        .cursors
        .all()
        .iter()
        .map(|c| c.position)
        .collect();
    assert_eq!(positions, vec![2, 6]);
}
