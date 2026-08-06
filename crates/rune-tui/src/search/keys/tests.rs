//! Unit tests for `search/keys.rs`'s keystroke handling — split into its
//! own file (500-line budget), mirroring how `search/tests.rs` already
//! splits `search/mod.rs`'s own tests out; a child module of `keys`, so
//! every private item there stays reachable through `use super::*;` exactly
//! as if this were still inline.

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Mem;

use super::*;
use crate::app::App;

fn app_with(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.frame_width = 80;
    app.frame_height = 24;
    app.sync_view();
    app
}

fn char_key(c: char) -> KeyInput {
    KeyInput {
        code: KeyCode::Char(c),
        mods: Mods::NONE,
    }
}

#[test]
fn typing_recomputes_matches_live() {
    let mut app = app_with("hello world hello");
    crate::search::open(&mut app);
    let mut effects = Effects::default();

    for c in "hello".chars() {
        assert_eq!(
            handle_key(&mut app, char_key(c), &mut effects),
            KeyOutcome::Consumed
        );
    }

    let state = app.search.as_ref().expect("bar stays open");
    assert_eq!(state.draft, "hello");
    assert_eq!(state.matches, vec![0..5, 12..17]);
}

#[test]
fn backspace_on_an_empty_draft_leaves_the_bar_open() {
    let mut app = app_with("hello");
    crate::search::open(&mut app);
    let mut effects = Effects::default();

    let backspace = KeyInput {
        code: KeyCode::Backspace,
        mods: Mods::NONE,
    };
    assert_eq!(
        handle_key(&mut app, backspace, &mut effects),
        KeyOutcome::Consumed
    );
    assert!(
        app.search.is_some(),
        "an empty-draft Backspace must not close the bar"
    );
}

#[test]
fn backspace_erases_one_grapheme_and_clears_its_matches() {
    let mut app = app_with("ab ab");
    crate::search::open(&mut app);
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('a'), &mut effects);
    let _ = handle_key(&mut app, char_key('b'), &mut effects);
    assert_eq!(app.search.as_ref().unwrap().matches, vec![0..2, 3..5]);

    let backspace = KeyInput {
        code: KeyCode::Backspace,
        mods: Mods::NONE,
    };
    let _ = handle_key(&mut app, backspace, &mut effects);
    let state = app.search.as_ref().unwrap();
    assert_eq!(state.draft, "a");
    assert!(!state.matches.is_empty());
}

#[test]
fn escape_closes_the_bar_and_saves_the_query() {
    let mut app = app_with("hello");
    crate::search::open(&mut app);
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('h'), &mut effects);

    let esc = KeyInput {
        code: KeyCode::Escape,
        mods: Mods::NONE,
    };
    assert_eq!(
        handle_key(&mut app, esc, &mut effects),
        KeyOutcome::Consumed
    );
    assert!(app.search.is_none(), "Escape closes the bar");
    assert_eq!(app.last_search_query.as_deref(), Some("h"));
}

#[test]
fn arrow_keys_with_empty_history_leave_state_untouched() {
    let mut app = app_with("hello");
    crate::search::open(&mut app);
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('h'), &mut effects);
    let before = app.search.as_ref().unwrap().draft.clone();

    for code in [KeyCode::Up, KeyCode::Down] {
        let key = KeyInput {
            code,
            mods: Mods::NONE,
        };
        assert_eq!(
            handle_key(&mut app, key, &mut effects),
            KeyOutcome::Consumed
        );
        assert_eq!(app.search.as_ref().unwrap().draft, before);
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
    crate::search::open(&mut app);
    let mut effects = Effects::default();
    for c in "hi".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }
    assert_eq!(app.search.as_ref().unwrap().matches, vec![0..2, 3..5, 6..8]);

    // The cursor starts at 0, inside the first match, so Enter three
    // times over must visit 1, 2, then wrap back to 0.
    let _ = handle_key(&mut app, enter_key(), &mut effects);
    assert_eq!(app.search.as_ref().unwrap().current, Some(1));
    let _ = handle_key(&mut app, enter_key(), &mut effects);
    assert_eq!(app.search.as_ref().unwrap().current, Some(2));
    assert_eq!(app.active_doc().cursors.primary().position, 6);
    let _ = handle_key(&mut app, enter_key(), &mut effects);
    assert_eq!(app.search.as_ref().unwrap().current, Some(0));
    assert_eq!(app.active_doc().cursors.primary().position, 0);
}

#[test]
fn shift_enter_wraps_from_the_first_match_to_the_last() {
    let mut app = app_with("hi hi hi");
    crate::search::open(&mut app);
    let mut effects = Effects::default();
    for c in "hi".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }

    let _ = handle_key(&mut app, shift_enter_key(), &mut effects);
    let state = app.search.as_ref().unwrap();
    assert_eq!(state.current, Some(2));
    assert_eq!(app.active_doc().cursors.primary().position, 6);
}

#[test]
fn enter_with_zero_matches_is_a_consumed_no_op() {
    let mut app = app_with("hello");
    crate::search::open(&mut app);
    let mut effects = Effects::default();
    for c in "zzz".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }
    assert!(app.search.as_ref().unwrap().matches.is_empty());
    let cursor_before = app.active_doc().cursors.primary().position;

    assert_eq!(
        handle_key(&mut app, enter_key(), &mut effects),
        KeyOutcome::Consumed
    );
    assert_eq!(app.search.as_ref().unwrap().current, None);
    assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
}

#[test]
fn enter_skips_matches_fully_inside_a_concealed_table_separator_but_still_counts_them() {
    // A leading paragraph keeps the default cursor (offset 0) OUTSIDE
    // the table below — reveal-on-cursor un-conceals whatever element
    // the cursor sits inside, and a cursor left inside the table itself
    // would defeat this fixture entirely.
    let mut app = app_with("text\n\n| a | b |\n|---|---|\n| a | c |\n");
    crate::search::open(&mut app);
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('-'), &mut effects);

    let state = app.search.as_ref().unwrap();
    assert!(!state.matches.is_empty(), "N still counts every '-'");
    let concealed = state.concealed.clone();
    assert!(
        state.matches.iter().all(|m| is_concealed(&concealed, m)),
        "every '-' sits inside the substituted separator row"
    );

    let cursor_before = app.active_doc().cursors.primary().position;
    let _ = handle_key(&mut app, enter_key(), &mut effects);
    assert_eq!(
        app.search.as_ref().unwrap().current,
        None,
        "every match is concealed, so navigation finds nothing to land on"
    );
    assert_eq!(app.active_doc().cursors.primary().position, cursor_before);
}

#[test]
fn read_only_document_scrolls_the_viewport_on_a_jump() {
    let content: String = (0..200).map(|i| format!("line {i} needle\n")).collect();
    let mut app = app_with(&content);
    app.active_doc_mut().read_only = crate::document::ReadOnly::Always;
    app.active_doc_mut().viewport.set_size(80, 10);
    app.sync_view();
    crate::search::open(&mut app);
    let mut effects = Effects::default();
    for c in "line 150".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }
    assert!(!app.search.as_ref().unwrap().matches.is_empty());
    let scroll_before = app.active_doc().viewport.scroll_row;

    let _ = handle_key(&mut app, enter_key(), &mut effects);

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
    crate::search::open(&mut app);
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('h'), &mut effects);
    let _ = handle_key(&mut app, char_key('i'), &mut effects);

    assert_eq!(
        handle_key(&mut app, enter_key(), &mut effects),
        KeyOutcome::Consumed
    );
    assert_eq!(app.search.as_ref().unwrap().current, Some(1));
    assert_eq!(app.last_search_query.as_deref(), Some("hi"));
    assert_eq!(
        messages::newest_text(&app),
        None,
        "a degraded store skips the write entirely, so there is nothing to report"
    );
}

fn up_key() -> KeyInput {
    KeyInput {
        code: KeyCode::Up,
        mods: Mods::NONE,
    }
}

fn down_key() -> KeyInput {
    KeyInput {
        code: KeyCode::Down,
        mods: Mods::NONE,
    }
}

#[test]
fn up_filters_history_against_the_currently_typed_draft() {
    let mut app = app_with("hello");
    crate::search::open(&mut app);
    app.search.as_mut().unwrap().history = vec![
        "needle".to_string(),
        "hay".to_string(),
        "haystack".to_string(),
    ];
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('h'), &mut effects);
    let _ = handle_key(&mut app, char_key('a'), &mut effects);

    let _ = handle_key(&mut app, up_key(), &mut effects);

    // "needle" has no "h" at all, so it's filtered out; "hay" is the
    // MRU-most surviving entry, so the first ↑ lands there rather than
    // "haystack".
    assert_eq!(app.search.as_ref().unwrap().draft, "hay");
}

#[test]
fn up_walks_older_in_mru_order_and_clamps_at_the_oldest() {
    let mut app = app_with("hello");
    crate::search::open(&mut app);
    app.search.as_mut().unwrap().history = vec!["one".to_string(), "two".to_string()];
    let mut effects = Effects::default();

    let _ = handle_key(&mut app, up_key(), &mut effects);
    assert_eq!(app.search.as_ref().unwrap().draft, "one");
    let _ = handle_key(&mut app, up_key(), &mut effects);
    assert_eq!(app.search.as_ref().unwrap().draft, "two");
    // Already at the oldest entry — a further ↑ clamps rather than
    // wrapping back around to "one".
    let _ = handle_key(&mut app, up_key(), &mut effects);
    assert_eq!(app.search.as_ref().unwrap().draft, "two");
}

#[test]
fn down_past_the_newest_entry_restores_the_in_progress_draft() {
    let mut app = app_with("hello");
    crate::search::open(&mut app);
    app.search.as_mut().unwrap().history = vec!["hello world".to_string(), "help".to_string()];
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('h'), &mut effects);

    let _ = handle_key(&mut app, up_key(), &mut effects);
    assert_eq!(app.search.as_ref().unwrap().draft, "hello world");

    let _ = handle_key(&mut app, down_key(), &mut effects);
    assert_eq!(
        app.search.as_ref().unwrap().draft,
        "h",
        "walking down past the newest entry restores the pre-browse draft"
    );
    assert!(app.search.as_ref().unwrap().history_pos.is_none());
}

#[test]
fn down_with_no_browse_session_active_is_a_no_op() {
    let mut app = app_with("hello");
    crate::search::open(&mut app);
    app.search.as_mut().unwrap().history = vec!["one".to_string()];
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('x'), &mut effects);

    let _ = handle_key(&mut app, down_key(), &mut effects);
    assert_eq!(app.search.as_ref().unwrap().draft, "x");
}

#[test]
fn typing_after_browsing_history_resets_the_browse_session() {
    let mut app = app_with("hello");
    crate::search::open(&mut app);
    app.search.as_mut().unwrap().history = vec!["one".to_string()];
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, up_key(), &mut effects);
    assert_eq!(app.search.as_ref().unwrap().draft, "one");

    let _ = handle_key(&mut app, char_key('!'), &mut effects);
    assert_eq!(app.search.as_ref().unwrap().draft, "one!");
    assert!(app.search.as_ref().unwrap().history_pos.is_none());
}

#[test]
fn a_ctrl_modified_char_is_swallowed_rather_than_typed() {
    let mut app = app_with("hello");
    crate::search::open(&mut app);
    let mut effects = Effects::default();

    let ctrl_x = KeyInput {
        code: KeyCode::Char('x'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    };
    assert_eq!(
        handle_key(&mut app, ctrl_x, &mut effects),
        KeyOutcome::Consumed
    );
    assert_eq!(app.search.as_ref().unwrap().draft, "");
}

// --- Closed-bar next/prev (`GlobalCommand::SearchNext`/`SearchPrev`,
// plan WP5) — driven through `pane::handle_global_command`, the actual
// dispatch entry point for these chords, rather than calling
// `advance_closed` directly, so the tests exercise the same path a real
// keypress would.

#[test]
fn closed_bar_next_steps_and_wraps_using_the_last_query() {
    let mut app = app_with("hi hi hi");
    crate::search::open(&mut app);
    let mut effects = Effects::default();
    for c in "hi".chars() {
        let _ = handle_key(&mut app, char_key(c), &mut effects);
    }
    let esc = KeyInput {
        code: KeyCode::Escape,
        mods: Mods::NONE,
    };
    let _ = handle_key(&mut app, esc, &mut effects);
    assert!(app.search.is_none(), "the bar is closed for this test");
    assert_eq!(app.last_search_query.as_deref(), Some("hi"));

    crate::pane::handle_global_command(
        &mut app,
        crate::keymap::GlobalCommand::SearchNext,
        &mut effects,
    );
    assert_eq!(app.active_doc().cursors.primary().position, 3);
    crate::pane::handle_global_command(
        &mut app,
        crate::keymap::GlobalCommand::SearchNext,
        &mut effects,
    );
    assert_eq!(app.active_doc().cursors.primary().position, 6);
    crate::pane::handle_global_command(
        &mut app,
        crate::keymap::GlobalCommand::SearchNext,
        &mut effects,
    );
    assert_eq!(
        app.active_doc().cursors.primary().position,
        0,
        "next wraps from the last match back to the first"
    );
    assert!(
        app.search.is_none(),
        "closed-bar navigation never reopens the bar"
    );
}

#[test]
fn closed_bar_prev_wraps_from_the_first_match_to_the_last() {
    let mut app = app_with("hi hi hi");
    crate::search::open(&mut app);
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
        app.active_doc().cursors.primary().position,
        6,
        "prev from the first match wraps to the last"
    );
}

#[test]
fn last_query_survives_closing_and_reopening_the_bar() {
    let mut app = app_with("hi hi");
    crate::search::open(&mut app);
    let mut effects = Effects::default();
    let _ = handle_key(&mut app, char_key('h'), &mut effects);
    let _ = handle_key(&mut app, char_key('i'), &mut effects);
    let esc = KeyInput {
        code: KeyCode::Escape,
        mods: Mods::NONE,
    };
    let _ = handle_key(&mut app, esc, &mut effects);

    // Reopening starts a fresh, empty draft (`search::open`'s own
    // contract) — it must never seed from `last_search_query`, but the
    // field itself must survive so a subsequent closed-bar chord still
    // has something to navigate with.
    crate::search::open(&mut app);
    let _ = handle_key(&mut app, esc, &mut effects);
    assert_eq!(app.last_search_query.as_deref(), Some("hi"));

    crate::pane::handle_global_command(
        &mut app,
        crate::keymap::GlobalCommand::SearchNext,
        &mut effects,
    );
    assert_eq!(app.active_doc().cursors.primary().position, 3);
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
