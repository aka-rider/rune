//! ↑/↓ history-browsing tests for `search/keys.rs` — split out of the
//! sibling `tests` module (500-line budget); basic keystroke editing has
//! its own further sibling, `editing_tests`. A child module of `keys`, so
//! every private item there stays reachable through `use super::*;`
//! exactly as if this were still inline.

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
