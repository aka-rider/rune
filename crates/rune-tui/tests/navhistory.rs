#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod navhistory_common;

use std::path::Path;
use std::sync::Arc;

use rune_tui::keymap::KeyCode;
use rune_vfs::{Mem, VfsTestExt};

use navhistory_common::*;

#[test]
fn link_follow_then_back_returns_and_forward_returns_again() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/note.md"), b"note body\n")
        .expect("seed note.md");
    let content = "[[note]]\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    let link_offset = content.find("note").expect("fixture has note");
    place_cursor(&mut app, link_offset);

    press_and_open(&mut app, sup_enter());
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/note.md"))
    );

    press(&mut app, back_key());
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/a.md"))
    );
    assert_eq!(
        app.active_doc().cursors.primary().position.get(),
        link_offset
    );

    press(&mut app, forward_key());
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/note.md"))
    );
}

#[test]
fn search_jump_then_back_returns_to_the_starting_caret() {
    let mut content = String::from("start\n");
    for i in 0..14 {
        content.push_str(&format!("filler {i}\n"));
    }
    content.push_str("NEEDLE ahead\n");
    let mem = Arc::new(Mem::new());
    let mut app = app_with(&mem, "/root/a.md", &content);
    let start_offset = app.active_doc().cursors.primary().position;

    press(&mut app, ctrl(KeyCode::Char('f')));
    for c in "NEEDLE".chars() {
        press(&mut app, plain(KeyCode::Char(c)));
    }
    press(&mut app, plain(KeyCode::Enter));
    let jumped = app.active_doc().cursors.primary().position;
    assert!(jumped > start_offset, "the search jump must move the caret");

    press(&mut app, back_key());
    assert_eq!(app.active_doc().cursors.primary().position, start_offset);
}

#[test]
fn five_consecutive_backs_never_grow_the_entry_list() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/note.md"), b"note body\n")
        .expect("seed note.md");
    let content = "[[note]]\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    place_cursor(&mut app, content.find("note").expect("fixture has note"));
    press_and_open(&mut app, sup_enter());

    press(&mut app, back_key());
    let stable_len = app.nav_history.len();
    assert!(stable_len >= 1);

    for _ in 0..4 {
        press(&mut app, back_key());
        assert_eq!(
            app.nav_history.len(),
            stable_len,
            "further backs must never grow the entry list"
        );
    }
}

#[test]
fn a_thirty_line_jump_records_while_a_five_line_move_does_not() {
    let content = numbered_lines(40);
    let mut app = plain_app(&content, WIDTH, 50);
    place_cursor(&mut app, 0);

    click(&mut app, 0, 5);
    assert_eq!(app.nav_history.len(), 0);

    click(&mut app, 0, 35);
    assert_eq!(app.nav_history.len(), 1);
}

#[test]
fn an_entry_below_an_insertion_still_lands_on_the_same_text() {
    let mut lines: Vec<String> = (0..20).map(|i| format!("line {i}\n")).collect();
    lines[15] = "TARGET\n".to_string();
    let content: String = lines.concat();
    let target_offset = content.find("TARGET").expect("fixture has TARGET");
    let mut app = plain_app(&content, WIDTH, 40);
    place_cursor(&mut app, target_offset);

    click(&mut app, 0, 0);
    assert_eq!(app.nav_history.len(), 1);

    press(&mut app, plain(KeyCode::Char('X')));

    press(&mut app, back_key());
    press(&mut app, back_key());

    let landed = app.active_doc().cursors.primary().position.get();
    let buffer = app.active_doc().buffer.content();
    assert_eq!(&buffer[landed..landed + "TARGET".len()], "TARGET");
}

#[test]
fn forward_at_the_boundary_moves_nothing_and_stays_silent() {
    let mut app = plain_app("hello\n", WIDTH, HEIGHT);
    let before = app.active;
    let cursor_before = app.active_doc().cursors.primary().position;
    assert!(!app.nav_history.can_forward());

    press(&mut app, forward_key());

    assert_eq!(rune_tui::messages::newest_text(&app), None);
    assert!(!app.nav_history.can_forward());
    assert_eq!(app.active, before);
    assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
}

/// At a boundary the dim control on the breadcrumb row IS the feedback:
/// the key moves nothing and says nothing.
#[test]
fn back_at_the_boundary_moves_nothing_and_stays_silent() {
    let mut app = plain_app("hello\n", WIDTH, HEIGHT);
    let before = app.active;
    let cursor_before = app.active_doc().cursors.primary().position;
    assert!(!app.nav_history.can_back());

    press(&mut app, back_key());

    assert_eq!(rune_tui::messages::newest_text(&app), None);
    assert!(!app.nav_history.can_back());
    assert_eq!(app.active, before);
    assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
    assert_eq!(app.nav_history.index(), 0);
    assert_eq!(app.nav_history.len(), 0);
}
