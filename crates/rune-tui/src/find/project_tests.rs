use rune_core::buffer::Buffer;

use super::bindings::FindCommand;
use super::test_support::*;
use super::*;
use crate::keymap::{KeyCode, Mods};
use crate::runtime::{Effects, Msg};

fn footer_helps(app: &App) -> Vec<String> {
    crate::footer_hints::default_hint_entries(app)
        .iter()
        .map(|(_, help, _)| help.to_string())
        .collect()
}

fn chip_fg(app: &mut App, needle: &str) -> Option<ratatui::style::Color> {
    app.sync_view();
    let rows = crate::testgrid::grid(app, PROJECT_W, PROJECT_H);
    let (y, row) = rows
        .iter()
        .enumerate()
        .find(|(_, row)| row.contains("This file"))
        .expect("the This-file chip is on screen");
    let x = occurrences(row, needle)
        .first()
        .copied()
        .expect("chip half");
    crate::testgrid::draw(app, PROJECT_W, PROJECT_H)
        .cell((x, y as u16))
        .and_then(|cell| cell.style().fg)
}

#[test]
fn ctrl_shift_f_opens_the_workspace_filter_with_the_this_file_chip_off() {
    let mut app = seeded_app(&[("/root/a.md", b"needle"), ("/root/b.md", b"needle")]);
    assert!(!app.splits.left.is_shown(), "test setup: column hidden");
    let mut effects = Effects::default();

    search_project(&mut app, "needle", &mut effects);

    assert_eq!(find(&app).filter_kind(), FilterKind::Workspace);
    assert!(find(&app).focused);
    assert_eq!(find(&app).focus, Control::Find);
    assert_eq!(
        chip_fg(&mut app, "This file"),
        app.theme.chrome.footer_key_inactive.fg
    );
    let rows = project_grid(&mut app);
    assert!(
        rows.iter().any(|row| row.contains(" Search ")),
        "two matching files force the left column open with the results title"
    );
}

#[test]
fn ctrl_f_opens_the_this_file_filter_with_the_chip_on() {
    let mut app = seeded_app(&[("/root/a.md", b"needle")]);

    press(&mut app, key(KeyCode::Char('f'), CTRL));

    assert_eq!(find(&app).filter_kind(), FilterKind::ThisFile);
    assert_eq!(
        chip_fg(&mut app, "This file"),
        app.theme.chrome.footer_key.fg
    );
}

#[test]
fn sup_shift_f_again_closes_the_panel_and_returns_to_the_origin() {
    let mut app = seeded_app(&[]);
    let second = app.open_document(Buffer::new("second"));
    crate::workspace::switch_to(&mut app, second);

    press(&mut app, key(KeyCode::Char('F'), SUP));
    assert_eq!(find(&app).filter_kind(), FilterKind::Workspace);

    press(&mut app, key(KeyCode::Char('F'), SUP));

    assert!(app.find().is_none());
    assert_eq!(app.active, second);
    assert_eq!(
        crate::focus::target(&app),
        crate::focus::FocusTarget::Editor
    );
}

#[test]
fn escape_closes_the_panel_and_discards_a_pending_preview() {
    let mut app = seeded_app(&[("/root/a.md", b"needle")]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    assert!(app.explorer.preview_awaiting.is_some());

    press_into(&mut app, esc_key(), &mut effects);

    assert!(app.find().is_none());
    assert!(app.explorer.preview_awaiting.is_none());
    assert!(
        crate::layout::geometry(app.frame_area(), &app)
            .left_block
            .is_none(),
        "the forced column collapses again"
    );
}

#[test]
fn opening_over_the_file_finder_tears_it_down_first() {
    let mut app = seeded_app(&[]);
    let second = app.open_document(Buffer::new("second"));
    crate::workspace::switch_to(&mut app, second);
    press(&mut app, key(KeyCode::Char('o'), SUP));
    assert!(app.filesearch().is_some(), "test setup: finder open");

    open_project_find(&mut app);

    assert!(app.filesearch().is_none());
    assert_eq!(find(&app).filter_kind(), FilterKind::Workspace);
    assert_eq!(
        app.active, second,
        "the finder's return_to must be restored before the panel records its origin"
    );
}

#[test]
fn a_close_bars_global_closes_a_project_scope_panel() {
    let mut app = seeded_app(&[]);
    open_project_find(&mut app);

    press(&mut app, key(KeyCode::F1, Mods::NONE));

    assert!(app.find().is_none());
}

#[test]
fn a_paste_lands_in_the_find_field_not_the_editor() {
    let mut app = seeded_app(&[]);
    let mut effects = open_project_find(&mut app);

    crate::app::update(
        &mut app,
        Msg::Paste("grep\nsecond line".to_string()),
        &mut effects,
    );

    assert_eq!(
        find(&app).find.editor.text(),
        "grep",
        "only the first pasted line survives sanitization"
    );
    assert_eq!(app.active_doc().buffer().content(), "hello");
}

#[test]
fn the_readout_counts_files_and_matches_for_a_two_file_fixture() {
    let mut app = seeded_app(&[
        ("/root/a.md", b"needle needle needle"),
        ("/root/b.md", b"needle needle"),
    ]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);

    let readout = project_readout_row(&mut app);
    assert!(readout.contains("2 files \u{b7} 5"), "{readout:?}");
}

#[test]
fn the_readout_says_no_matches_for_a_missing_word() {
    let mut app = seeded_app(&[("/root/a.md", b"nothing here")]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);

    assert!(project_readout_row(&mut app).contains("no matches"));
}

#[test]
fn alt_f_turns_the_this_file_chip_on_keeps_the_query_and_selects_the_first_in_file_match() {
    let mut app = seeded_app(&[("/root/a.md", b"needle")]);
    let active = app.active;
    crate::commands::edit::insert_text(
        &mut app,
        active,
        "one needle two needle",
        rune_core::undo::EditKind::Other,
    );
    app.active_doc_mut()
        .set_cursors(rune_core::cursor::CursorSet::new(0));
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    assert_eq!(
        find(&app).current,
        Some(0),
        "the workspace filter follows a hit in the active file like This file does"
    );

    press_into(&mut app, key(KeyCode::Char('f'), ALT), &mut effects);

    assert_eq!(find(&app).filter_kind(), FilterKind::ThisFile);
    assert!(find(&app).project().is_none());
    assert_eq!(find(&app).find.editor.text(), "needle");
    assert_eq!(find(&app).current, Some(0));
    assert_eq!(selection_start(&app), 4);
    assert_eq!(
        chip_fg(&mut app, "This file"),
        app.theme.chrome.footer_key.fg
    );

    press_into(&mut app, key(KeyCode::Char('f'), ALT), &mut effects);
    assert_eq!(find(&app).filter_kind(), FilterKind::Workspace);
    assert_eq!(find(&app).find.editor.text(), "needle");
}

#[test]
fn the_workspace_footer_lists_results_and_open_and_no_globals() {
    let mut app = seeded_app(&[("/root/a.md", b"needle"), ("/root/b.md", b"needle")]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);

    let entries = crate::footer_hints::default_hint_entries(&app);
    let helps = footer_helps(&app);
    assert!(!helps.iter().any(|h| h == "save"), "{helps:?}");
    assert!(!helps.iter().any(|h| h == "quit"), "{helps:?}");
    assert!(helps.iter().any(|h| h == "open"), "{helps:?}");
    assert!(
        entries.iter().any(|(label, help, _)| {
            *label == crate::find::bindings::label_for(FindCommand::NextControl)
                && *help == "include"
        }),
        "{entries:?}"
    );
    assert!(helps.iter().any(|h| h == "next file"), "{helps:?}");
    assert!(helps.iter().any(|h| h == "replace in project"), "{helps:?}");
}

#[test]
fn the_tab_ring_reaches_the_results_and_arrows_move_the_selection() {
    let mut app = seeded_app(&[("/root/a.md", b"needle needle"), ("/root/b.md", b"needle")]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);

    press_into(&mut app, shift_tab(), &mut effects);
    assert_eq!(find(&app).focus, Control::Results);
    let helps = footer_helps(&app);
    assert!(helps.iter().any(|h| h == "open"), "{helps:?}");

    press_into(&mut app, down(), &mut effects);
    assert_eq!(project(&app).list.cursor, 1);
    press_into(&mut app, up(), &mut effects);
    assert_eq!(project(&app).list.cursor, 0);
    press_into(&mut app, key(KeyCode::End, Mods::NONE), &mut effects);
    assert_eq!(project(&app).list.cursor, 1);
    press_into(&mut app, key(KeyCode::Home, Mods::NONE), &mut effects);
    assert_eq!(project(&app).list.cursor, 0);
    press_into(&mut app, key(KeyCode::PageDown, Mods::NONE), &mut effects);
    assert_eq!(project(&app).list.cursor, 1);

    press_into(&mut app, tab(), &mut effects);
    assert_eq!(
        find(&app).focus,
        Control::Find,
        "the ring wraps back to Find"
    );
}

#[test]
fn typing_on_the_results_tells_the_user_how_to_open() {
    let mut app = seeded_app(&[("/root/a.md", b"needle"), ("/root/b.md", b"needle")]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    press_into(&mut app, shift_tab(), &mut effects);

    press_into(&mut app, char_key('x'), &mut effects);

    assert_eq!(find(&app).find.editor.text(), "needle");
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("press \u{23ce} to open the result")
    );
}

#[test]
fn list_keys_in_the_find_field_give_feedback_instead_of_vanishing() {
    let mut app = seeded_app(&[("/root/a.md", b"needle"), ("/root/b.md", b"needle")]);
    press(&mut app, key(KeyCode::Char('f'), CTRL));
    press(&mut app, key(KeyCode::PageDown, Mods::NONE));
    assert!(
        crate::messages::newest_text(&app).is_some_and(|text| text.contains("2+ files")),
        "{:?}",
        crate::messages::newest_text(&app)
    );

    press(&mut app, esc_key());
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    press_into(&mut app, key(KeyCode::PageUp, Mods::NONE), &mut effects);
    assert!(
        crate::messages::newest_text(&app).is_some_and(|text| text.contains("reaches them")),
        "{:?}",
        crate::messages::newest_text(&app)
    );
}

// Home/End are list-paging keys ONLY once the panel command table has no
// field claim left on them — with a field focused they move the caret
// instead, in every filter, Workspace included; only Results itself still
// reads them as list-paging (`the_tab_ring_reaches_the_results_and_arrows_
// move_the_selection` covers that half).
#[test]
fn home_in_a_focused_field_moves_the_caret_instead_of_paging_results_even_in_project_scope() {
    let mut app = seeded_app(&[("/root/a.md", b"needle")]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    assert_eq!(find(&app).focus, Control::Find);

    press_into(&mut app, key(KeyCode::Home, Mods::NONE), &mut effects);

    assert_eq!(find(&app).find.editor.cursor().position.get(), 0);
    assert_eq!(find(&app).find.editor.text(), "needle");
}

#[test]
fn ctrl_g_steps_to_the_next_result_file_and_opens_it() {
    let mut app = seeded_app(&[("/root/a.md", b"needle needle"), ("/root/b.md", b"needle")]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    crate::explorer_preview::discard(&mut app);

    press_into(&mut app, key(KeyCode::Char('g'), CTRL), &mut effects);
    assert_eq!(project(&app).list.cursor, 1);
    assert_eq!(
        app.active_doc().path(),
        Some(std::path::Path::new("/root/b.md"))
    );
    assert!(find(&app).focused, "the panel stays open and focused");

    press_into(&mut app, key(KeyCode::Char('g'), CTRL), &mut effects);
    assert_eq!(project(&app).list.cursor, 0, "next wraps to the first hit");
    assert_eq!(
        app.active_doc().path(),
        Some(std::path::Path::new("/root/a.md"))
    );
}

#[test]
fn ctrl_f_on_a_project_panel_switches_it_to_file_scope_and_back() {
    let mut app = seeded_app(&[]);
    open_project_find(&mut app);

    press(&mut app, key(KeyCode::Char('f'), CTRL));
    assert_eq!(find(&app).filter_kind(), FilterKind::ThisFile);
    assert!(find(&app).focused);

    open_project_find(&mut app);
    assert_eq!(find(&app).filter_kind(), FilterKind::Workspace);

    open_project_find(&mut app);
    assert!(
        app.find().is_none(),
        "the chord that matches the current filter closes a focused panel"
    );
}

#[test]
fn ctrl_shift_r_expands_replace_in_the_workspace_filter() {
    let mut app = seeded_app(&[]);
    press(&mut app, key(KeyCode::Char('R'), CTRL));

    assert_eq!(find(&app).filter_kind(), FilterKind::Workspace);
    assert!(find(&app).replace.is_some());
    assert_eq!(find(&app).focus, Control::Replace);
    assert_eq!(
        find(&app).control_ring(),
        vec![
            Control::Find,
            Control::Replace,
            Control::Include,
            Control::Exclude
        ]
    );
}

#[test]
fn tab_visits_find_replace_globs_and_results_in_order_when_two_files_match() {
    let mut app = seeded_app(&[("/root/a.md", b"needle"), ("/root/b.md", b"needle")]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    press_into(&mut app, key(KeyCode::Char('R'), CTRL), &mut effects);
    assert_eq!(find(&app).focus, Control::Replace);

    for expected in [
        Control::Include,
        Control::Exclude,
        Control::Results,
        Control::Find,
    ] {
        press(&mut app, tab());
        assert_eq!(find(&app).focus, expected);
    }
    press(&mut app, tab());
    assert_eq!(find(&app).focus, Control::Replace);
}

#[test]
fn enter_with_no_results_reports_instead_of_vanishing() {
    let mut app = seeded_app(&[("/root/a.md", b"nothing")]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);

    press_into(&mut app, enter(), &mut effects);

    assert_eq!(
        crate::messages::newest_text(&app),
        Some("no matches for \"needle\" in the project")
    );
    assert!(find(&app).focused);
}
