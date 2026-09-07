use std::sync::Arc;

use rune_vfs::Mem;

use super::test_support::*;
use super::*;
use crate::keymap::KeyCode;
use crate::runtime::{CmdKind, Msg, RecentsResult};

fn in_memory_db() -> crate::db::Db {
    crate::db::Db::new(
        rune_db::Store::open_in_memory(
            Arc::new(std::time::SystemTime::now),
            Arc::new(Mem::new()),
            Box::new(|_evt| {}),
        )
        .expect("open in-memory store"),
        crate::db::DbBridge::bootstrap(),
        false,
    )
}

fn open_replace_for(app: &mut App, needle: &str, replacement: &str) {
    open_find(app);
    type_str(app, needle);
    open_replace(app);
    type_str(app, replacement);
}

fn replace_recents(app: &mut App, generation: u64, entries: &[&str]) {
    let mut effects = Effects::default();
    crate::app::update(
        app,
        Msg::RecentsLoaded {
            generation,
            result: RecentsResult::Replace(Ok(entries.iter().map(|e| e.to_string()).collect())),
        },
        &mut effects,
    );
}

fn replace_field(app: &App) -> &FieldState {
    find(app)
        .replace
        .as_ref()
        .expect("the replace row is expanded")
}

fn content(app: &App) -> &str {
    app.active_doc().buffer.content()
}

#[test]
fn enter_in_replace_replaces_only_the_current_match_and_selects_the_next() {
    let mut app = app_with("dog cat dog cat dog");
    open_replace_for(&mut app, "dog", "bird");
    assert_eq!(find(&app).current, Some(0));

    press(&mut app, enter());

    assert_eq!(content(&app), "bird cat dog cat dog");
    assert_eq!(find(&app).matches, vec![9..12, 17..20]);
    assert_eq!(find(&app).current, Some(0));
    assert_eq!(selection_start(&app), 9);
    assert!(app.active_doc().cursors.primary().has_selection());
    assert!(find(&app).focused);
    assert_eq!(find(&app).focus, Control::Replace);
}

#[test]
fn shift_enter_replaces_every_match_and_one_undo_restores_the_original_bytes() {
    let original = "dog cat dog cat dog\nDOG\n";
    let mut app = app_with(original);
    open_replace_for(&mut app, "dog", "bird");

    press(&mut app, shift_enter());

    assert_eq!(content(&app), "bird cat bird cat bird\nbird\n");
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("replaced 4 occurrences")
    );
    assert!(find(&app).matches.is_empty());
    assert!(find(&app).focused, "the panel stays focused");

    press(&mut app, escape());
    press(&mut app, key(KeyCode::Char('z'), SUP));
    assert_eq!(content(&app), original);
}

#[test]
fn reading_mode_refuses_a_replace_without_claiming_success() {
    let mut app = app_with("dog cat dog");
    press(&mut app, key(KeyCode::Char('P'), CTRL));
    assert!(app.active_doc().is_read_only(), "reading view is on");
    open_replace_for(&mut app, "dog", "bird");
    let version = app.active_doc().buffer.version();

    press(&mut app, enter());
    press(&mut app, shift_enter());

    assert_eq!(content(&app), "dog cat dog");
    assert_eq!(app.active_doc().buffer.version(), version);
    let newest = crate::messages::newest_text(&app).unwrap_or_default();
    assert!(!newest.contains("replaced"), "{newest:?}");
    assert!(newest.contains("reading view"), "{newest:?}");
}

#[test]
fn regex_capture_groups_expand_in_the_replacement() {
    let mut app = app_with("bob@ amy@");
    open_find(&mut app);
    type_str(&mut app, r"(\w+)@");
    press(&mut app, key(KeyCode::Char('r'), ALT));
    assert_eq!(find(&app).matches, vec![0..4, 5..9]);
    open_replace(&mut app);
    type_str(&mut app, "$1 at");

    press(&mut app, shift_enter());

    assert_eq!(content(&app), "bob at amy at");
}

#[test]
fn text_mode_writes_dollar_sequences_verbatim() {
    let mut app = app_with("price");
    open_replace_for(&mut app, "price", "$1");

    press(&mut app, enter());

    assert_eq!(content(&app), "$1");
}

#[test]
fn the_last_replacement_reports_no_more_matches_and_settles_the_origin_there() {
    let mut app = app_with("dog cat");
    open_replace_for(&mut app, "dog", "bird");

    press(&mut app, enter());

    assert_eq!(content(&app), "bird cat");
    assert_eq!(crate::messages::newest_text(&app), Some("no more matches"));
    assert!(find(&app).matches.is_empty());
    assert!(find(&app).focused);

    press(&mut app, escape());
    assert_eq!(
        app.active_doc().cursors.primary().position.get(),
        4,
        "escape lands after the replacement, not on the pre-replace cursor"
    );
}

#[test]
fn enter_in_replace_with_no_current_match_selects_one_before_editing() {
    let mut app = app_with("x dog y dog");
    open_replace_for(&mut app, "dog", "cat");
    let geo = crate::layout::geometry(app.frame_area(), &app);
    let panel = geo.find_panel.expect("panel open");
    click(&mut app, geo.editor.x, geo.editor.y);
    press(&mut app, char_key('z'));
    assert_eq!(content(&app), "zx dog y dog");
    let replace_field_rect = panel.replace_field.expect("replace row shown");
    click(&mut app, replace_field_rect.x, replace_field_rect.y);
    assert_eq!(find(&app).focus, Control::Replace);

    press(&mut app, enter());
    assert_eq!(
        content(&app),
        "zx dog y dog",
        "the first Enter only selects"
    );
    assert_eq!(find(&app).current, Some(0));
    assert_eq!(selection_start(&app), 3);

    press(&mut app, enter());
    assert_eq!(content(&app), "zx cat y dog");
}

#[test]
fn replace_all_with_nothing_to_replace_says_so() {
    let mut app = app_with("cat");
    open_replace_for(&mut app, "dog", "x");
    let version = app.active_doc().buffer.version();

    press(&mut app, shift_enter());
    press(&mut app, enter());

    assert_eq!(content(&app), "cat");
    assert_eq!(app.active_doc().buffer.version(), version);
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("no match to replace")
    );
}

#[test]
fn space_on_the_replace_buttons_replaces_one_then_all() {
    let mut app = app_with("dog dog dog");
    open_replace_for(&mut app, "dog", "cat");

    press(&mut app, tab());
    assert_eq!(find(&app).focus, Control::ReplaceOne);
    press(&mut app, space());
    assert_eq!(content(&app), "cat dog dog");

    press(&mut app, tab());
    assert_eq!(find(&app).focus, Control::ReplaceAll);
    press(&mut app, space());
    assert_eq!(content(&app), "cat cat cat");
}

#[test]
fn clicking_the_all_chip_replaces_every_match() {
    let mut app = app_with("dog dog");
    open_replace_for(&mut app, "dog", "cat");
    let panel = crate::layout::geometry(app.frame_area(), &app)
        .find_panel
        .expect("panel open");
    let (chip, rect) = panel.chips[5].expect("the All chip is laid out");
    assert_eq!(chip.control, Control::ReplaceAll);

    click(&mut app, rect.x, rect.y);

    assert_eq!(content(&app), "cat cat");
}

#[test]
fn the_footer_in_the_replace_field_advertises_replace_keys() {
    let mut app = app_with("dog");
    open_replace_for(&mut app, "dog", "cat");
    let rows = grid(&mut app);
    let footer = rows.last().cloned().unwrap_or_default();
    for expected in [
        "\u{23ce} replace",
        "\u{21e7}\u{23ce} replace all",
        "^G skip",
    ] {
        assert!(
            footer.contains(expected),
            "{expected:?} missing in {footer:?}"
        );
    }
}

#[test]
fn the_replace_field_has_its_own_history() {
    let mut app = app_with("dog");
    open_replace(&mut app);
    let generation = find(&app).replace_history_generation.raw();
    replace_recents(&mut app, generation, &["cat"]);
    assert_eq!(replace_field(&app).history, vec!["cat".to_string()]);

    press(&mut app, up());
    assert_eq!(replace_field(&app).draft, "cat");

    press(&mut app, key(KeyCode::Char('f'), CTRL));
    assert_eq!(find(&app).focus, Control::Find);
    press(&mut app, up());
    assert_eq!(
        find(&app).find.draft,
        "",
        "the Find field never offers replace text"
    );
}

#[test]
fn a_replace_history_reply_lands_only_on_its_own_panel_generation() {
    let mut app = app_with("dog");
    open_replace(&mut app);
    let stale = find(&app).replace_history_generation;
    press(&mut app, escape());
    open_replace(&mut app);
    let live = find(&app).replace_history_generation;
    assert_ne!(stale, live);

    replace_recents(&mut app, stale.raw(), &["old"]);
    assert!(replace_field(&app).history.is_empty());

    replace_recents(&mut app, live.raw(), &["new"]);
    assert_eq!(replace_field(&app).history, vec!["new".to_string()]);
}

#[test]
fn a_failed_replace_history_load_reports_and_leaves_the_field_usable() {
    let mut app = app_with("dog");
    open_replace(&mut app);
    let generation = find(&app).replace_history_generation.raw();
    let mut effects = Effects::default();
    crate::app::update(
        &mut app,
        Msg::RecentsLoaded {
            generation,
            result: RecentsResult::Replace(Err(crate::runtime::CmdError::Refused(
                "reader gone".to_string(),
            ))),
        },
        &mut effects,
    );

    assert!(replace_field(&app).history.is_empty());
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("replace history not loaded: reader gone")
    );
    type_str(&mut app, "x");
    assert_eq!(replace_field(&app).draft, "x");
}

#[test]
fn expanding_replace_requests_the_replace_history_exactly_once() {
    let mut app = app_with("dog");
    app.db = Some(in_memory_db());

    let effects = press(&mut app, key(KeyCode::Char('f'), CTRL));
    assert_eq!(effects.cmds.len(), 1, "the find history load");

    let effects = press(&mut app, key(KeyCode::Char('r'), CTRL));
    assert_eq!(effects.cmds.len(), 1);
    assert_eq!(effects.cmds[0].kind(), CmdKind::SearchHistory);

    let effects = press(&mut app, key(KeyCode::Char('r'), CTRL));
    assert!(
        effects.cmds.is_empty(),
        "an already expanded row loads nothing"
    );
}

#[test]
fn a_replace_persists_the_query_and_the_replacement_separately() {
    let mut app = app_with("dog dog");
    app.db = Some(in_memory_db());
    open_replace_for(&mut app, "dog", "cat");

    press(&mut app, enter());

    assert_eq!(app.search_history.last_persisted.as_deref(), Some("dog"));
    assert_eq!(app.replace_history.last_persisted.as_deref(), Some("cat"));
}

#[test]
fn an_empty_replacement_deletes_the_match_and_persists_no_replace_history() {
    let mut app = app_with("a dog b");
    app.db = Some(in_memory_db());
    open_replace_for(&mut app, "dog", "");

    press(&mut app, enter());

    assert_eq!(content(&app), "a  b");
    assert_eq!(app.replace_history.last_persisted, None);
}
