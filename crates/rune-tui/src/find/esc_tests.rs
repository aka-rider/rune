use rune_core::buffer::Buffer;

use super::test_support::*;
use super::*;
use crate::pane::Pane;

fn click_into_editor(app: &mut App) {
    let geo = crate::layout::geometry(app.frame_area(), app);
    click(app, geo.editor.x, geo.editor.y);
}

#[test]
fn esc_from_the_editor_closes_a_kept_panel_before_the_old_ladder_runs() {
    let mut app = app_with("hello world");
    open_find(&mut app);
    type_str(&mut app, "world");
    click_into_editor(&mut app);
    press(&mut app, shift_right());
    let selected = app.active_doc().cursors().clone();
    assert!(
        !find(&app).focused,
        "test setup: the click unfocused the panel"
    );

    press(&mut app, esc_key());

    assert!(app.find().is_none(), "Esc closes the panel it left behind");
    assert_eq!(
        *app.active_doc().cursors(),
        selected,
        "the cursor stays put"
    );
    assert_ne!(app.focus(), Pane::Explorer);

    press(&mut app, esc_key());
    assert!(
        !app.active_doc().cursors().primary().has_selection(),
        "the second Esc collapses the selection"
    );
    assert_ne!(app.focus(), Pane::Explorer);

    press(&mut app, esc_key());
    assert_eq!(
        app.focus(),
        Pane::Explorer,
        "the third Esc reaches the Explorer"
    );
}

#[test]
fn esc_in_a_focused_panel_restores_the_origin_cursor() {
    let mut app = app_with("hello world");
    open_find(&mut app);
    type_str(&mut app, "world");
    assert_eq!(selection_start(&app), 6);

    press(&mut app, esc_key());

    assert!(app.find().is_none());
    assert_eq!(selection_start(&app), 0);
    assert_eq!(app.active_doc().cursors().primary().position.get(), 0);
}

#[test]
fn esc_after_a_tab_switch_closes_the_panel_and_stays_on_the_new_tab() {
    let mut app = app_with("alpha");
    let first = app.active;
    let second = app.open_document(Buffer::new("bravo"));
    open_find(&mut app);
    type_str(&mut app, "alpha");

    let second_idx = app
        .documents
        .order()
        .iter()
        .position(|&id| id == second)
        .expect("second tab is listed");
    assert!(crate::workspace::select_tab(&mut app, second_idx));
    assert_eq!(app.active, second);
    assert!(!find(&app).focused, "navigating away unfocuses the panel");
    assert_eq!(find(&app).origin.doc, second);

    press(&mut app, esc_key());

    assert!(app.find().is_none());
    assert_eq!(app.active, second, "Esc must not pull the user back");
    assert_ne!(app.active, first);
}

#[test]
fn esc_does_not_restore_an_origin_cursor_into_a_buffer_edited_since() {
    let mut app = app_with("hello world");
    open_find(&mut app);
    type_str(&mut app, "world");
    click_into_editor(&mut app);
    press(&mut app, end());
    for _ in 0.."hello world".len() {
        press(&mut app, backspace());
    }
    assert_eq!(app.active_doc().buffer().content(), "", "test setup");
    open_find(&mut app);
    assert!(find(&app).focused, "test setup: the panel has focus again");

    press(&mut app, esc_key());

    assert!(app.find().is_none());
    assert_eq!(app.active_doc().cursors().primary().position.get(), 0);
}
