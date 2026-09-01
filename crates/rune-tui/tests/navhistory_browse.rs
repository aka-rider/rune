#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod navhistory_common;

use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app;
use rune_tui::keymap::KeyCode;
use rune_tui::pane::Pane;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

use navhistory_common::explorer_common;
use navhistory_common::*;

#[test]
fn opening_a_second_file_from_the_explorer_records_the_first() {
    let mem = explorer_common::seeded_vfs();
    let mut app = browsing_app(&mem);
    let first = app.active;

    arrow_to(&mut app, "b.md");
    press_and_settle(&mut app, plain(KeyCode::Enter));

    assert_eq!(active_path(&app).as_deref(), Some(Path::new("/root/b.md")));
    assert!(
        app.nav_history.can_back(),
        "opening a file from the Explorer must record where the user left"
    );

    press(&mut app, back_key());

    assert_eq!(app.active, first);
}

#[test]
fn reopening_an_already_open_file_from_the_explorer_records_the_departure() {
    let mem = explorer_common::seeded_vfs();
    let mut app = browsing_app(&mem);
    let first = app.active;
    arrow_to(&mut app, "b.md");
    press_and_settle(&mut app, plain(KeyCode::Enter));
    let second = app.active;

    focus_explorer(&mut app);
    arrow_to(&mut app, "a.md");
    press_and_settle(&mut app, plain(KeyCode::Enter));

    assert_eq!(app.active, first, "a.md was already open");

    press(&mut app, back_key());

    assert_eq!(app.active, second);
}

#[test]
fn escaping_the_explorer_onto_a_preview_records_the_departure() {
    let mem = explorer_common::seeded_vfs();
    let mut app = browsing_app(&mem);
    let first = app.active;

    arrow_to(&mut app, "b.md");
    press_and_settle(&mut app, plain(KeyCode::Escape));

    assert_eq!(active_path(&app).as_deref(), Some(Path::new("/root/b.md")));
    assert!(app.explorer.preview.is_none(), "Escape commits the preview");

    press(&mut app, back_key());

    assert_eq!(app.active, first);
}

#[test]
fn clicking_into_the_editor_from_the_explorer_records_the_departure() {
    let mem = explorer_common::seeded_vfs();
    let mut app = browsing_app(&mem);
    let first = app.active;

    arrow_to(&mut app, "b.md");
    click(&mut app, 0, 0);

    assert_eq!(active_path(&app).as_deref(), Some(Path::new("/root/b.md")));

    press(&mut app, back_key());

    assert_eq!(app.active, first);
}

#[test]
fn reopening_the_file_you_are_already_in_records_nothing() {
    let mem = explorer_common::seeded_vfs();
    let mut app = browsing_app(&mem);
    let first = app.active;

    arrow_to(&mut app, "a.md");
    press_and_settle(&mut app, plain(KeyCode::Enter));

    assert_eq!(app.active, first);
    assert_eq!(app.nav_history.len(), 0);
    assert!(!app.nav_history.can_back());
}

#[test]
fn a_failed_explorer_open_records_nothing() {
    let mem = explorer_common::seeded_vfs();
    let mut app = browsing_app(&mem);
    mem.fail_resolve(Path::new("/root/b.md"));

    arrow_to(&mut app, "b.md");
    press_and_settle(&mut app, plain(KeyCode::Enter));

    assert_ne!(active_path(&app).as_deref(), Some(Path::new("/root/b.md")));
    assert_eq!(app.nav_history.len(), 0);
}

#[test]
fn travelling_back_and_forward_from_the_explorer_pane_never_records_the_preview() {
    let mem = explorer_common::seeded_vfs();
    let mut app = browsing_app(&mem);
    let first = app.active;
    arrow_to(&mut app, "b.md");
    press_and_settle(&mut app, plain(KeyCode::Enter));
    let second = app.active;

    focus_explorer(&mut app);
    arrow_to(&mut app, "sub");
    press_and_settle(&mut app, plain(KeyCode::Enter));
    arrow_to(&mut app, "c.md");
    assert!(
        app.explorer.preview.is_some(),
        "a live preview to travel over"
    );
    let preview = app.explorer.preview.as_ref().expect("a live preview").id;

    press(&mut app, back_key());
    assert_eq!(app.active, first);
    assert!(
        app.explorer.preview.is_none(),
        "travelling away discards the preview"
    );

    press(&mut app, forward_key());

    assert_eq!(
        app.active, second,
        "forward returns to the file browsing started from"
    );
    assert!(
        app.nav_history
            .places()
            .iter()
            .all(|place| place.doc != preview),
        "a preview must never be recorded as a place travel can reach"
    );
}

#[test]
fn a_live_explorer_preview_never_becomes_a_place_travel_can_land_on() {
    let mem = explorer_common::seeded_vfs();
    let mut app = browsing_app(&mem);
    arrow_to(&mut app, "b.md");
    press_and_settle(&mut app, plain(KeyCode::Enter));

    focus_explorer(&mut app);
    arrow_to(&mut app, "sub");
    press_and_settle(&mut app, plain(KeyCode::Enter));
    arrow_to(&mut app, "c.md");
    assert!(
        app.explorer.preview.is_some(),
        "a live preview to travel over"
    );
    let preview = app.explorer.preview.as_ref().expect("a live preview").id;

    press(&mut app, back_key());
    press(&mut app, forward_key());

    assert!(
        app.nav_history
            .places()
            .iter()
            .all(|place| place.doc != preview),
        "a preview must never be recorded as a place"
    );
    assert!(
        app.explorer.preview.is_none(),
        "travel discards the preview it travelled over"
    );
}

#[test]
fn reopening_an_already_open_file_from_the_finder_records_the_departure() {
    let mem = explorer_common::seeded_vfs();
    let mut app = app_with(&mem, "/root/a.md", "a content");
    let first = app.active;
    let second =
        rune_tui::workspace::open_path(&mut app, Path::new("/root/b.md")).expect("open b.md");
    assert_eq!(app.active, second);

    press_and_settle(&mut app, ctrl(KeyCode::Char('o')));
    for c in "a.md".chars() {
        press_and_settle(&mut app, plain(KeyCode::Char(c)));
    }
    press_and_settle(&mut app, plain(KeyCode::Enter));

    assert_eq!(app.active, first, "a.md was already open");

    press(&mut app, back_key());

    assert_eq!(app.active, second);
}

#[test]
fn arrowing_the_explorer_preview_records_nothing() {
    let mem = explorer_common::seeded_vfs();
    let mut app = explorer_common::app_with(&mem);
    explorer_common::load_explorer(&mut app);

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Key(explorer_common::key(KeyCode::Down)),
        &mut effects,
    );
    settle_file_opens(&mut app, effects);
    assert!(app.explorer.preview.is_some());
    assert_eq!(app.nav_history.len(), 0);

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Key(explorer_common::key(KeyCode::Down)),
        &mut effects,
    );
    settle_file_opens(&mut app, effects);
    assert_eq!(app.nav_history.len(), 0);
}

#[test]
fn ctrl_1_tab_switch_records_the_departure() {
    let mem = Arc::new(Mem::new());
    let mut app = app_with(&mem, "/root/a.md", "a content\n");
    let a_id = app.active;
    let b_id = app.open_document(Buffer::new("b content\n"));
    rune_tui::workspace::switch_to(&mut app, b_id);
    assert_eq!(app.active, b_id);

    press(&mut app, ctrl(KeyCode::Char('1')));

    assert_eq!(app.active, a_id);
    assert!(app.nav_history.can_back());

    press(&mut app, back_key());

    assert_eq!(app.active, b_id);
}

#[test]
fn selecting_a_tab_from_the_tabs_pane_records_the_departure() {
    let mem = Arc::new(Mem::new());
    let mut app = app_with(&mem, "/root/a.md", "a content\n");
    let a_id = app.active;
    let b_id = app.open_document(Buffer::new("b content\n"));
    rune_tui::workspace::switch_to(&mut app, b_id);

    press(&mut app, ctrl(KeyCode::Char('t')));
    press(&mut app, plain(KeyCode::Up));
    press(&mut app, plain(KeyCode::Enter));
    assert_eq!(app.active, a_id);

    press(&mut app, back_key());

    assert_eq!(app.active, b_id);
}

/// A frame too narrow to paint the Editor resolves Escape's focus move back
/// to the Explorer: nothing is promoted, nothing is switched, so nothing may
/// be recorded — an entry here would light `^[ back` up for a navigation the
/// user never made and yank them out of the browse.
#[test]
fn escaping_a_narrow_explorer_records_nothing() {
    let mem = explorer_common::seeded_vfs();
    let mut app = browsing_app(&mem);
    arrow_to(&mut app, "b.md");
    assert!(app.explorer.preview.is_some());

    app.frame = Some(rune_tui::app::FrameSize::new(24, app.frame_height()));
    app.sync_view();
    press_and_settle(&mut app, plain(KeyCode::Escape));

    assert_eq!(
        app.focus(),
        Pane::Explorer,
        "a narrow frame keeps focus on the Explorer"
    );
    assert!(
        app.explorer.preview.is_some(),
        "the preview was never promoted"
    );
    assert_eq!(app.nav_history.len(), 0);
    assert!(!app.nav_history.can_back());
}

#[test]
fn switching_tabs_while_a_preview_is_live_records_the_browsed_from_file() {
    let mem = explorer_common::seeded_vfs();
    let mut app = browsing_app(&mem);
    let first = app.active;
    arrow_to(&mut app, "b.md");
    press_and_settle(&mut app, plain(KeyCode::Enter));
    let second = app.active;

    focus_explorer(&mut app);
    arrow_to(&mut app, "sub");
    press_and_settle(&mut app, plain(KeyCode::Enter));
    arrow_to(&mut app, "c.md");
    assert!(
        app.explorer.preview.is_some(),
        "a live preview to switch off"
    );

    press_and_settle(&mut app, ctrl(KeyCode::Char('1')));
    assert_eq!(app.active, first, "^1 selects the first tab");

    press(&mut app, back_key());

    assert_eq!(
        app.active, second,
        "the departure is the file browsing started from, never the preview"
    );
}

#[test]
fn a_new_document_records_the_departure() {
    let mem = Arc::new(Mem::new());
    let mut app = app_with(&mem, "/root/a.md", "a content\n");
    let first = app.active;

    press_and_settle(&mut app, ctrl(KeyCode::Char('n')));
    assert_ne!(app.active, first, "^n mints and activates a draft");
    press(&mut app, plain(KeyCode::Escape));

    press(&mut app, back_key());

    assert_eq!(app.active, first);
}

#[test]
fn a_new_document_while_a_preview_is_live_records_the_browsed_from_file() {
    let mem = explorer_common::seeded_vfs();
    let mut app = browsing_app(&mem);
    let first = app.active;
    arrow_to(&mut app, "b.md");
    assert!(
        app.explorer.preview.is_some(),
        "a live preview to mint over"
    );

    press_and_settle(&mut app, ctrl(KeyCode::Char('n')));
    press(&mut app, plain(KeyCode::Escape));

    press(&mut app, back_key());

    assert_eq!(
        app.active, first,
        "the departure is the file browsing started from, never the preview"
    );
}
