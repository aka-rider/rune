//! Basic keystroke-editing tests for `search/keys.rs` — typing, erasing,
//! closing, and an unbound chord — split out of the sibling `tests` module
//! (500-line budget); ↑/↓ history browsing has its own further sibling,
//! `history_tests`. A child module of `keys`, so every private item there
//! stays reachable through `use super::*;` exactly as if this were still
//! inline.

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
