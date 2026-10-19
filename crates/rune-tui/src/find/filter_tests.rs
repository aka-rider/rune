use super::test_support::*;
use super::*;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::runtime::Effects;

fn ctrl(c: char) -> KeyInput {
    key(KeyCode::Char(c), CTRL)
}

fn alt(c: char) -> KeyInput {
    key(KeyCode::Char(c), ALT)
}

fn two_files() -> App {
    seeded_app(&[("/root/a.md", b"needle"), ("/root/b.md", b"needle")])
}

#[test]
fn ctrl_f_searches_only_the_active_document_and_never_builds_the_workspace_index() {
    let mut app = seeded_app(&[("/root/a.md", b"needle needle needle")]);
    let active = app.active;
    crate::commands::edit::insert_text(
        &mut app,
        active,
        " needle",
        rune_core::undo::EditKind::Other,
    );
    let mut effects = Effects::default();

    press_into(&mut app, ctrl('f'), &mut effects);
    type_into(&mut app, "needle", &mut effects);

    assert_eq!(find(&app).filter_kind(), FilterKind::ThisFile);
    assert_eq!(find(&app).matches.len(), 1, "only the draft's own hit");
    assert!(app.project_index.is_none());
    assert!(!app.left_column_overlay());
}

#[test]
fn ctrl_shift_f_opens_the_workspace_filter_and_ctrl_f_switches_it_back_without_closing() {
    let mut app = two_files();
    let mut effects = Effects::default();

    press_into(&mut app, ctrl('F'), &mut effects);
    assert_eq!(find(&app).filter_kind(), FilterKind::Workspace);

    press_into(&mut app, ctrl('f'), &mut effects);
    assert_eq!(find(&app).filter_kind(), FilterKind::ThisFile);
    assert!(find(&app).focused);
    assert_eq!(find(&app).focus, Control::Find);
}

#[test]
fn alt_f_toggles_the_filter_and_requeries_the_new_engine() {
    let mut app = two_files();
    let mut effects = Effects::default();
    press_into(&mut app, ctrl('f'), &mut effects);
    type_into(&mut app, "hell", &mut effects);
    assert_eq!(find(&app).matches.len(), 1);

    press_into(&mut app, alt('f'), &mut effects);
    assert_eq!(find(&app).filter_kind(), FilterKind::Workspace);
    fire_debounce(&mut app, &mut effects);
    pump_index(&mut app, &mut effects);
    fire_debounce(&mut app, &mut effects);
    deliver_query(&mut app, &mut effects);
    assert_eq!(project(&app).results.len(), 0, "the workspace has no hell");

    press_into(&mut app, alt('f'), &mut effects);
    assert_eq!(find(&app).filter_kind(), FilterKind::ThisFile);
    assert_eq!(find(&app).matches.len(), 1, "the file engine ran again");
}

#[test]
fn the_results_list_is_hidden_for_zero_and_one_matching_file_and_shown_for_two() {
    for (corpus, query, listed) in [
        (vec![("/root/a.md", &b"nothing"[..])], "needle", false),
        (vec![("/root/a.md", &b"needle"[..])], "needle", false),
        (
            vec![
                ("/root/a.md", &b"needle"[..]),
                ("/root/b.md", &b"needle"[..]),
            ],
            "needle",
            true,
        ),
    ] {
        let mut app = seeded_app(&corpus);
        let mut effects = Effects::default();

        search_project(&mut app, query, &mut effects);

        assert_eq!(app.left_column_overlay(), listed, "{corpus:?}");
        assert_eq!(
            find(&app).control_ring().contains(&Control::Results),
            listed,
            "{corpus:?}"
        );
        let rows = project_grid(&mut app);
        assert_eq!(
            rows.iter().any(|row| row.contains(" Search ")),
            listed,
            "{corpus:?}"
        );
    }
}

#[test]
fn the_ring_has_no_results_stop_while_the_query_is_too_short_to_search() {
    let mut app = two_files();
    let mut effects = Effects::default();
    press_into(&mut app, ctrl('F'), &mut effects);
    type_into(&mut app, "n", &mut effects);

    assert_eq!(
        find(&app).control_ring(),
        vec![Control::Find, Control::Include, Control::Exclude]
    );
    press_into(&mut app, shift_tab(), &mut effects);
    assert_eq!(find(&app).focus, Control::Exclude);
}

#[test]
fn focus_leaves_the_results_when_the_list_hides() {
    let mut app = two_files();
    let mut effects = Effects::default();
    search_project(&mut app, "NEEDLE", &mut effects);
    press_into(&mut app, shift_tab(), &mut effects);
    assert_eq!(find(&app).focus, Control::Results, "test setup");

    press_into(&mut app, alt('c'), &mut effects);
    fire_debounce(&mut app, &mut effects);
    deliver_query(&mut app, &mut effects);

    assert_eq!(project(&app).results.len(), 0);
    assert_eq!(find(&app).focus, Control::Find);
}

#[test]
fn turning_this_file_back_on_while_on_the_results_moves_focus_to_find() {
    let mut app = two_files();
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    press_into(&mut app, shift_tab(), &mut effects);
    assert_eq!(find(&app).focus, Control::Results, "test setup");

    press_into(&mut app, alt('f'), &mut effects);

    assert_eq!(find(&app).filter_kind(), FilterKind::ThisFile);
    assert_eq!(find(&app).focus, Control::Find);
    assert!(!app.left_column_overlay());
}

#[test]
fn modifier_free_f_in_the_panel_is_text_not_the_chip() {
    let mut app = two_files();
    let mut effects = Effects::default();
    press_into(&mut app, ctrl('f'), &mut effects);

    press_into(&mut app, key(KeyCode::Char('f'), Mods::NONE), &mut effects);

    assert_eq!(find(&app).filter_kind(), FilterKind::ThisFile);
    assert_eq!(find(&app).find.editor.text(), "f");
}
