use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Mem;

use super::*;
use crate::app::App;

fn app_with(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.frame = Some(crate::app::FrameSize::new(80, 24));
    app.sync_view();
    app
}

fn char_key(c: char) -> KeyInput {
    KeyInput {
        code: KeyCode::Char(c),
        mods: Mods::NONE,
    }
}

fn enter_key() -> KeyInput {
    KeyInput {
        code: KeyCode::Enter,
        mods: Mods::NONE,
    }
}

fn shift_enter_key() -> KeyInput {
    KeyInput {
        code: KeyCode::Enter,
        mods: SHIFT,
    }
}

#[test]
fn enter_wraps_from_the_last_match_to_the_first() {
    let mut app = app_with("hi hi hi");
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    for c in "hi".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }
    assert_eq!(app.search().unwrap().matches, vec![0..2, 3..5, 6..8]);

    let _ = handle_key(&mut app, enter_key(), &mut effects);
    assert_eq!(app.search().unwrap().current, Some(1));
    let _ = handle_key(&mut app, enter_key(), &mut effects);
    assert_eq!(app.search().unwrap().current, Some(2));
    assert_eq!(app.active_doc().cursors.primary().position.get(), 6);
    let _ = handle_key(&mut app, enter_key(), &mut effects);
    assert_eq!(app.search().unwrap().current, Some(0));
    assert_eq!(app.active_doc().cursors.primary().position.get(), 0);
}

#[test]
fn shift_enter_wraps_from_the_first_match_to_the_last() {
    let mut app = app_with("hi hi hi");
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    for c in "hi".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }

    let _ = handle_key(&mut app, shift_enter_key(), &mut effects);
    let state = app.search().unwrap();
    assert_eq!(state.current, Some(2));
    assert_eq!(app.active_doc().cursors.primary().position.get(), 6);
}

#[test]
fn enter_with_zero_matches_is_a_consumed_no_op() {
    let mut app = app_with("hello");
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    for c in "zzz".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }
    assert!(app.search().unwrap().matches.is_empty());
    let cursor_before = app.active_doc().cursors.primary().position;

    assert_eq!(
        handle_key(&mut app, enter_key(), &mut effects),
        KeyOutcome::Consumed
    );
    assert_eq!(app.search().unwrap().current, None);
    assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
    assert_eq!(
        messages::newest_text(&app),
        Some("no matches for \"zzz\""),
        "a query with no matches must say so, not fail silently"
    );
}

#[test]
fn enter_skips_matches_fully_inside_a_concealed_table_separator_but_still_counts_them() {
    let mut app = app_with("text\n\n| a | b |\n|---|---|\n| a | c |\n");
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('-'), &mut effects);

    let state = app.search().unwrap();
    assert!(!state.matches.is_empty(), "N still counts every '-'");
    let matches = state.matches.clone();
    let concealed = current_concealed(&app);
    assert!(
        matches.iter().all(|m| is_concealed(&concealed, m)),
        "every '-' sits inside the substituted separator row"
    );

    let cursor_before = app.active_doc().cursors.primary().position;
    let _ = handle_key(&mut app, enter_key(), &mut effects);
    assert_eq!(
        app.search().unwrap().current,
        None,
        "every match is concealed, so navigation finds nothing to land on"
    );
    assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
    assert_eq!(
        messages::newest_text(&app),
        Some(format!("all {} matches are concealed", matches.len())).as_deref(),
        "a concealed-only match list must say so, not fail silently"
    );
}

#[test]
fn revealing_the_table_makes_its_matches_navigable_without_a_buffer_edit() {
    let mut app = app_with("text\n\n| a | b |\n|---|---|\n| a | c |\n");
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('-'), &mut effects);
    let version_before = app.active_doc().buffer.version();
    let _ = handle_key(&mut app, enter_key(), &mut effects);
    assert_eq!(
        app.search().unwrap().current,
        None,
        "still concealed before the cursor ever enters the table"
    );

    let table_offset = "text\n\n| a".len();
    app.active_doc_mut().cursors = CursorSet::new(table_offset);
    app.sync_view();
    assert_eq!(
        app.active_doc().buffer.version(),
        version_before,
        "revealing must never look like a buffer edit"
    );

    let _ = handle_key(&mut app, enter_key(), &mut effects);
    assert!(
        app.search().unwrap().current.is_some(),
        "the revealed row's matches must be navigable on the very next Enter"
    );
}

#[test]
fn read_only_document_scrolls_the_viewport_on_a_jump() {
    let content: String = (0..200).map(|i| format!("line {i} needle\n")).collect();
    let mut app = app_with(&content);
    app.active_doc_mut().read_only = crate::document::ReadOnly::Always;
    app.active_doc_mut().viewport.set_size(80, 10);
    app.sync_view();
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    for c in "line 150".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }
    assert!(!app.search().unwrap().matches.is_empty());
    let scroll_before = app.active_doc().viewport.scroll_row;

    let _ = handle_key(&mut app, enter_key(), &mut effects);
    app.sync_view();

    assert_ne!(
        app.active_doc().viewport.scroll_row,
        scroll_before,
        "a jump on a read-only document must move the viewport explicitly"
    );
}

#[test]
fn a_degraded_db_attempts_no_write_but_still_navigates() {
    let mut app = app_with("hi hi");
    app.db = Some(crate::db::Db::new(
        rune_db::Store::open_in_memory(
            Arc::new(std::time::SystemTime::now),
            Arc::new(Mem::new()),
            Box::new(|_evt| {}),
        )
        .expect("open in-memory store"),
        crate::db::DbBridge::bootstrap(),
        true,
    ));
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('h'), &mut effects);
    let _ = handle_key(&mut app, char_key('i'), &mut effects);

    assert_eq!(
        handle_key(&mut app, enter_key(), &mut effects),
        KeyOutcome::Consumed
    );
    assert_eq!(app.search().unwrap().current, Some(1));
    assert_eq!(app.last_search_query.as_deref(), Some("hi"));
    assert_eq!(
        messages::newest_text(&app),
        None,
        "a degraded store skips the write entirely, so there is nothing to report"
    );
}

#[test]
fn enter_after_a_coalesced_doc_switch_recomputes_instead_of_jumping_into_the_old_doc() {
    let mut app = app_with("needle needle");
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    for c in "needle".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }
    assert_eq!(app.search().unwrap().matches, vec![0..6, 7..13]);
    let stale_doc = app.search().unwrap().doc;

    let other = app.open_document(Buffer::new("no matches in here"));
    app.active = other;

    let _ = handle_key(&mut app, enter_key(), &mut effects);

    let state = app.search().unwrap();
    assert_ne!(
        state.doc, stale_doc,
        "the recompute must retarget the new active doc"
    );
    assert_eq!(state.doc, other);
    assert!(
        state.matches.is_empty(),
        "the new document has no \"needle\" — the stale match list must not survive"
    );
    assert_eq!(
        app.active_doc().cursors.primary().position.get(),
        0,
        "no jump into the wrong document's byte ranges"
    );
}

#[test]
fn repeated_enter_on_the_same_query_enqueues_one_touch_op() {
    let mut app = app_with("hi hi hi");
    app.db = Some(crate::db::Db::new(
        rune_db::Store::open_in_memory(
            Arc::new(std::time::SystemTime::now),
            Arc::new(Mem::new()),
            Box::new(|_evt| {}),
        )
        .expect("open in-memory store"),
        crate::db::DbBridge::bootstrap(),
        false,
    ));
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    for c in "hi".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }

    let _ = handle_key(&mut app, enter_key(), &mut effects);
    let _ = handle_key(&mut app, enter_key(), &mut effects);
    let _ = handle_key(&mut app, enter_key(), &mut effects);

    assert_eq!(
        app.search_history.ops.len(),
        1,
        "an unchanged query across repeated Enter must enqueue exactly one write"
    );
}

#[test]
fn closed_bar_next_steps_and_wraps_using_the_last_query() {
    let mut app = app_with("hi hi hi");
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    for c in "hi".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }
    let esc = KeyInput {
        code: KeyCode::Escape,
        mods: Mods::NONE,
    };
    let _ = handle_key(&mut app, esc, &mut effects);
    assert!(app.search().is_none(), "the bar is closed for this test");
    assert_eq!(app.last_search_query.as_deref(), Some("hi"));

    crate::pane::handle_global_command(
        &mut app,
        crate::keymap::GlobalCommand::SearchNext,
        &mut effects,
    );
    assert_eq!(app.active_doc().cursors.primary().position.get(), 3);
    crate::pane::handle_global_command(
        &mut app,
        crate::keymap::GlobalCommand::SearchNext,
        &mut effects,
    );
    assert_eq!(app.active_doc().cursors.primary().position.get(), 6);
    crate::pane::handle_global_command(
        &mut app,
        crate::keymap::GlobalCommand::SearchNext,
        &mut effects,
    );
    assert_eq!(
        app.active_doc().cursors.primary().position.get(),
        0,
        "next wraps from the last match back to the first"
    );
    assert!(
        app.search().is_none(),
        "closed-bar navigation never reopens the bar"
    );
}

#[test]
fn closed_bar_prev_wraps_from_the_first_match_to_the_last() {
    let mut app = app_with("hi hi hi");
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    for c in "hi".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }
    let esc = KeyInput {
        code: KeyCode::Escape,
        mods: Mods::NONE,
    };
    let _ = handle_key(&mut app, esc, &mut effects);

    crate::pane::handle_global_command(
        &mut app,
        crate::keymap::GlobalCommand::SearchPrev,
        &mut effects,
    );
    assert_eq!(
        app.active_doc().cursors.primary().position.get(),
        6,
        "prev from the first match wraps to the last"
    );
}

#[test]
fn last_query_survives_closing_and_reopening_the_bar() {
    let mut app = app_with("hi hi");
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('h'), &mut effects);
    let _ = handle_key(&mut app, char_key('i'), &mut effects);
    let esc = KeyInput {
        code: KeyCode::Escape,
        mods: Mods::NONE,
    };
    let _ = handle_key(&mut app, esc, &mut effects);

    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    let _ = handle_key(&mut app, esc, &mut effects);
    assert_eq!(app.last_search_query.as_deref(), Some("hi"));

    crate::pane::handle_global_command(
        &mut app,
        crate::keymap::GlobalCommand::SearchNext,
        &mut effects,
    );
    assert_eq!(app.active_doc().cursors.primary().position.get(), 3);
}

#[test]
fn no_last_query_reports_feedback_instead_of_a_silent_no_op() {
    let mut app = app_with("hello");
    let mut effects = Effects::default();
    assert!(app.last_search_query.is_none());

    crate::pane::handle_global_command(
        &mut app,
        crate::keymap::GlobalCommand::SearchNext,
        &mut effects,
    );

    assert_eq!(
        messages::newest_text(&app),
        Some("no previous search"),
        "an unreachable chord must still give feedback, never swallow the keypress"
    );
}
