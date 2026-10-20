use rune_core::buffer::Buffer;

use super::test_support::*;
use super::*;
use crate::pane::Pane;
use crate::runtime::Effects;

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

fn panel_kept_open_with(app: &mut App) {
    open_find(app);
    type_str(app, "world");
    click_into_editor(app);
    assert!(app.find().is_some_and(|state| !state.focused));
}

fn focus(app: &mut App, pane: Pane) {
    let mut effects = Effects::default();
    match pane {
        Pane::Explorer => crate::pane::show_and_focus_explorer_on_active_file(app, &mut effects),
        Pane::Tabs => crate::pane_global::focus_tabs(app, &mut effects),
        _ => app.set_focus_pane(pane, &mut effects),
    }
    assert_eq!(app.focus(), pane, "test setup: the pane took focus");
}

#[test]
fn esc_closes_the_panel_first_whichever_pane_has_focus() {
    for pane in [Pane::Explorer, Pane::Tabs] {
        let mut app = app_with("hello world");
        panel_kept_open_with(&mut app);
        focus(&mut app, pane);

        press(&mut app, esc_key());

        assert!(app.find().is_none(), "Esc in {pane:?} closes the panel");
        assert_eq!(app.focus(), pane, "and leaves that pane where it was");
    }
}

#[test]
fn esc_in_the_messages_pane_closes_the_panel_before_collapsing_the_messages() {
    let mut app = app_with("hello world");
    panel_kept_open_with(&mut app);
    crate::messages::info(&mut app, "something to read");
    let mut effects = Effects::default();
    crate::messages::toggle(&mut app, &mut effects);
    focus(&mut app, Pane::Messages);

    press(&mut app, esc_key());

    assert!(app.find().is_none());
    assert!(
        crate::messages::is_open(&app),
        "the second Esc collapses it"
    );
}

#[test]
fn esc_while_renaming_cancels_the_rename_and_keeps_the_panel() {
    let mut app = app_with("hello world");
    panel_kept_open_with(&mut app);
    focus(&mut app, Pane::Title);

    press(&mut app, esc_key());

    assert!(app.find().is_some(), "the in-progress edit ends first");
    assert_ne!(app.focus(), Pane::Title);

    press(&mut app, esc_key());
    assert!(app.find().is_none());
}

fn title_stuck_on_a_refused_rename_with_the_panel_left_open() -> App {
    let mut app = seeded_app(&[("/root/doc.md", b"hello")]);
    let id =
        crate::workspace::open_path(&mut app, std::path::Path::new("/root/doc.md")).expect("opens");
    crate::workspace::switch_to(&mut app, id);
    open_find(&mut app);
    click_into_editor(&mut app);
    press(&mut app, enter());
    press(&mut app, up());
    press(&mut app, up());
    assert_eq!(app.focus(), Pane::Title, "test setup: the title took focus");
    app.doc_mut(id).expect("doc").read_only = crate::read_only::ReadOnly::Always;
    press(&mut app, backspace());
    assert!(app.find().is_some_and(|state| !state.focused));
    app
}

#[test]
fn no_search_chord_hands_the_panel_the_keyboard_while_a_refused_rename_holds_the_title() {
    for chord in ['f', 'F', 'r', 'R'] {
        let mut app = title_stuck_on_a_refused_rename_with_the_panel_left_open();

        press(&mut app, key(crate::keymap::KeyCode::Char(chord), CTRL));

        assert_eq!(
            app.focus(),
            Pane::Title,
            "ctrl-{chord}: the refused rename keeps the title"
        );
        assert_ne!(
            crate::focus::target(&app),
            crate::focus::FocusTarget::Find,
            "ctrl-{chord}"
        );
    }
}

#[test]
fn clicking_into_the_panel_never_hands_it_the_keyboard_while_a_refused_rename_holds_the_title() {
    let mut app = title_stuck_on_a_refused_rename_with_the_panel_left_open();
    let geo = crate::layout::geometry(app.frame_area(), &app);
    let panel = geo.find_panel.expect("panel open").find_field;

    click(&mut app, panel.x, panel.y);

    assert_eq!(app.focus(), Pane::Title);
    assert_ne!(crate::focus::target(&app), crate::focus::FocusTarget::Find);
}
