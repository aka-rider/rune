use super::test_support::*;
use super::*;
use crate::commands::test_support::selecting;
use crate::keymap::KeyCode;
use crate::runtime::{Msg, RecentsResult};

fn footer_row(rows: &[String]) -> &str {
    rows.last().map_or("", String::as_str)
}

fn readout_row(app: &mut App) -> String {
    let top = panel_top_row(app);
    grid(app).get(top + 1).cloned().unwrap_or_default()
}

#[test]
fn ctrl_f_opens_a_three_row_bordered_panel_directly_above_the_footer() {
    let mut app = app_with("hello");
    open_find(&mut app);
    let rows = grid(&mut app);
    let top = usize::from(FRAME_H) - 4;
    assert!(
        rows[top].contains("\u{256d} Find"),
        "top row: {:?}",
        rows[top]
    );
    assert!(
        rows[top + 1].starts_with('\u{2502}'),
        "field row: {:?}",
        rows[top + 1]
    );
    assert!(
        rows[top + 2].starts_with('\u{2570}'),
        "bottom row: {:?}",
        rows[top + 2]
    );
    assert!(
        rows[top + 1].contains("[File|Project] [Aa] [Word] [.*]"),
        "chips on the find row: {:?}",
        rows[top + 1]
    );
    assert!(
        footer_row(&rows).contains("close"),
        "footer: {:?}",
        footer_row(&rows)
    );
}

#[test]
fn opening_the_messages_pane_keeps_the_panel_rows_where_they_were() {
    let mut app = app_with("hello");
    open_find(&mut app);
    let before = panel_top_row(&mut app);
    crate::messages::info(&mut app, "a note for the log");
    press(&mut app, key(KeyCode::Char('e'), CTRL));
    assert!(
        app.find().is_none(),
        "toggling messages is a bar-closing global"
    );

    let mut app = app_with("hello");
    crate::messages::info(&mut app, "a note for the log");
    press(&mut app, key(KeyCode::Char('e'), CTRL));
    open_find(&mut app);
    let after = panel_top_row(&mut app);
    assert_eq!(after, before);
    let rows = grid(&mut app);
    assert!(
        rows[before - 1].contains("a note for the log"),
        "the messages pane sits directly above the panel: {:?}",
        rows[before - 1]
    );
}

#[test]
fn typing_selects_the_first_match_after_the_cursor_and_paints_it_stronger() {
    let mut app = app_with("dog cat dog cat dog");
    open_find(&mut app);
    type_str(&mut app, "dog");

    assert_eq!(find(&app).matches, vec![0..3, 8..11, 16..19]);
    assert_eq!(find(&app).current, Some(0));
    assert_eq!(selection_start(&app), 0);
    assert!(readout_row(&mut app).contains("1/3"));

    let rows = grid(&mut app);
    let (y, row) = rows
        .iter()
        .enumerate()
        .find(|(_, row)| row.contains("dog cat dog"))
        .expect("the document row is on screen");
    let xs = occurrences(row, "dog");
    assert_eq!(xs.len(), 3);
    let current = app.theme.chrome.search_current_bg.bg;
    let other = app.theme.chrome.search_match_bg.bg;
    assert_eq!(bg_at(&mut app, xs[0], y as u16), current);
    assert_eq!(bg_at(&mut app, xs[1], y as u16), other);
    assert_eq!(bg_at(&mut app, xs[2], y as u16), other);
}

#[test]
fn a_typo_restores_the_origin_cursor_and_reads_no_matches() {
    let mut app = app_with("one dog two");
    open_find(&mut app);
    type_str(&mut app, "dog");
    assert_eq!(selection_start(&app), 4);

    type_str(&mut app, "x");
    assert_eq!(selection_start(&app), 0);
    assert!(!app.active_doc().cursors.primary().has_selection());
    assert!(readout_row(&mut app).contains("no matches"));
    assert!(app.find().is_some(), "the panel stays open on a miss");
}

#[test]
fn escape_restores_the_origin_cursor_and_scroll_row() {
    let content: String = (0..200).map(|i| format!("line {i} needle\n")).collect();
    let mut app = app_with(&content);
    open_find(&mut app);
    type_str(&mut app, "line 150");
    app.sync_view();
    assert_ne!(
        app.active_doc().viewport.scroll_row.0,
        0,
        "the match scrolled into view"
    );

    press(&mut app, escape());
    app.sync_view();
    assert!(app.find().is_none());
    assert_eq!(selection_start(&app), 0);
    assert_eq!(app.active_doc().viewport.scroll_row.0, 0);
}

#[test]
fn enter_commits_so_a_later_escape_returns_to_that_match() {
    let mut app = app_with("hi hi hi");
    open_find(&mut app);
    type_str(&mut app, "hi");
    press(&mut app, enter());
    assert_eq!(selection_start(&app), 3);

    press(&mut app, escape());
    assert_eq!(
        selection_start(&app),
        3,
        "escape returns to the committed match"
    );
}

#[test]
fn ctrl_f_closes_a_focused_panel_and_focuses_a_kept_one() {
    let mut app = app_with("hi hi");
    open_find(&mut app);
    type_str(&mut app, "hi");
    crate::find::unfocus(&mut app);
    assert!(!find(&app).focused);

    open_find(&mut app);
    assert!(find(&app).focused, "^F on a kept panel focuses it");
    assert_eq!(find(&app).focus, Control::Find);

    open_find(&mut app);
    assert!(app.find().is_none(), "^F on a focused panel closes it");
    assert_eq!(
        selection_start(&app),
        0,
        "closing with ^F keeps the current match"
    );
}

#[test]
fn a_single_line_selection_seeds_the_field_and_a_multi_line_one_does_not() {
    let mut app = app_with("dog\ndog");
    let id = app.active;
    selecting(&mut app, id, 0, 3);
    open_find(&mut app);
    assert_eq!(find(&app).find.draft, "dog");
    assert_eq!(
        find(&app).current,
        Some(0),
        "the seeded selection stays current"
    );
    press(&mut app, escape());

    selecting(&mut app, id, 0, 7);
    open_find(&mut app);
    assert_eq!(find(&app).find.draft, "");
}

#[test]
fn a_regex_dot_on_a_long_document_counts_every_match_and_renders() {
    let content: String = (0..2000).map(|_| "abcde\n").collect();
    let mut app = app_with(&content);
    open_find(&mut app);
    press(&mut app, key(KeyCode::Char('r'), ALT));
    type_str(&mut app, ".");
    assert_eq!(find(&app).matches.len(), 10_000);
    assert!(readout_row(&mut app).contains("1/10000"));
}

#[test]
fn opening_the_panel_seeds_an_empty_focused_draft_with_no_matches() {
    let mut app = app_with("hello");
    open_find(&mut app);
    let state = find(&app);
    assert!(state.focused);
    assert_eq!(state.find.draft, "");
    assert!(state.matches.is_empty());
    assert!(state.replace.is_none());

    close(&mut app, false);
    assert!(app.find().is_none());
    assert_eq!(app.last_find, None);
    close(&mut app, false);
    assert!(app.find().is_none());
}

#[test]
fn expanding_replace_never_clobbers_an_in_progress_draft() {
    let mut app = app_with("hello");
    open_find(&mut app);
    type_str(&mut app, "h");
    open_replace(&mut app);
    assert_eq!(find(&app).find.draft, "h");
    assert_eq!(find(&app).matches, vec![0..1]);
}

#[test]
fn switching_the_active_document_resets_the_match_set() {
    let mut app = app_with("hello hello");
    open_find(&mut app);
    type_str(&mut app, "hello");
    assert_eq!(find(&app).matches.len(), 2);

    let other = app.open_document(rune_core::buffer::Buffer::new("no match in this one"));
    app.active = other;
    sync(&mut app);

    assert!(find(&app).matches.is_empty());
    assert_eq!(find(&app).doc, other);
}

fn recents(app: &mut App, generation: u64, result: Result<Vec<String>, crate::runtime::CmdError>) {
    let mut effects = crate::runtime::Effects::default();
    crate::app::update(
        app,
        Msg::RecentsLoaded {
            generation,
            result: RecentsResult::Search(result),
        },
        &mut effects,
    );
}

#[test]
fn a_stale_generation_history_reply_is_discarded() {
    let mut app = app_with("hello");
    open_find(&mut app);
    let stale = find(&app).history_generation;
    press(&mut app, escape());
    open_find(&mut app);
    let live = find(&app).history_generation;
    assert_ne!(stale, live, "each open mints a fresh generation");

    recents(&mut app, stale.raw(), Ok(vec!["should not land".into()]));
    assert!(find(&app).find.history.is_empty());

    recents(&mut app, live.raw(), Ok(vec!["one".into(), "two".into()]));
    assert_eq!(
        find(&app).find.history,
        vec!["one".to_string(), "two".to_string()]
    );
}

#[test]
fn a_reader_failure_degrades_history_to_empty_and_reports_a_message() {
    let mut app = app_with("hello");
    open_find(&mut app);
    let generation = find(&app).history_generation.raw();
    app.find_mut().unwrap().find.history = vec!["stale".to_string()];

    recents(
        &mut app,
        generation,
        Err(crate::runtime::CmdError::Refused("reader gone".to_string())),
    );

    assert!(find(&app).find.history.is_empty());
    assert!(app.find().is_some(), "the panel itself keeps working");
    assert!(crate::messages::newest_text(&app).is_some());
}

#[test]
fn the_scope_chip_switches_the_open_panel_to_project_scope_and_keeps_the_query() {
    let mut app = app_with("hello");
    app.set_root(std::path::PathBuf::from("/root"));
    open_find(&mut app);
    type_str(&mut app, "hel");
    press(&mut app, tab());
    assert_eq!(find(&app).focus, Control::Scope);
    press(&mut app, space());

    assert_eq!(find(&app).scope(), Scope::Project);
    assert!(find(&app).focused);
    assert_eq!(find(&app).find.draft, "hel");
    assert_eq!(find(&app).control_ring().last(), Some(&Control::Results));
}

#[test]
fn an_unbound_key_reports_instead_of_vanishing() {
    let mut app = app_with("hello");
    open_find(&mut app);
    press(&mut app, key(KeyCode::Char('x'), CTRL));
    assert_eq!(find(&app).find.draft, "");
    assert!(
        crate::messages::newest_text(&app).is_some_and(|text| text.contains("not bound")),
        "{:?}",
        crate::messages::newest_text(&app)
    );
}

#[test]
fn typing_on_a_chip_tells_the_user_how_to_toggle_it() {
    let mut app = app_with("hello");
    open_find(&mut app);
    press(&mut app, tab());
    press(&mut app, char_key('q'));
    assert_eq!(find(&app).find.draft, "");
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("press \u{2423} to toggle")
    );
}

#[test]
fn command_v_spawns_a_pbpaste_cmd_tagged_for_the_panel() {
    let mut app = app_with("hello");
    open_find(&mut app);
    let effects = press(&mut app, key(KeyCode::Char('v'), SUP));
    assert_eq!(effects.cmds.len(), 1, "exactly one pbpaste read spawned");
    assert_eq!(
        effects.cmds[0].kind(),
        crate::runtime::CmdKind::ClipboardRead
    );
    assert!(find(&app).find.draft.is_empty());
}
