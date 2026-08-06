//! Keystroke handling for the focused search bar — reached from
//! `dispatch::handle_key`'s stage 3 whenever `focus::target` resolves to
//! `FocusTarget::SearchField`, ahead of the ordinary chrome-level `Pane`
//! match, since the bar is never itself a `Pane`.
//!
//! Every path returns [`KeyOutcome::Consumed`] — Title's own discipline
//! (`title/keys.rs`), required by the fuzzer's `PANE-NO-BLEED` invariant:
//! a keystroke aimed at the bar must never fall through and mutate the
//! document buffer underneath it.

use unicode_segmentation::UnicodeSegmentation;

use rune_core::cursor::CursorSet;

use crate::app::App;
use crate::commands::nav_scroll;
use crate::keymap::{KeyCode, KeyInput, KeyOutcome, Mods};
use crate::messages;
use crate::runtime::Effects;

use super::{close, is_concealed, next_index, prev_index, recompute};

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

/// `effects` is unused today — every path here mutates only `App::search`,
/// never spawns a `Cmd` — but kept in the signature so this matches every
/// other pane handler's shape (`title::keys::handle_key`,
/// `explorer_keys::handle_key`) and a later change (history load, match
/// persistence) can start using it without a signature change rippling
/// through `dispatch.rs`.
pub(crate) fn handle_key(app: &mut App, key: KeyInput, _effects: &mut Effects) -> KeyOutcome {
    match key.code {
        KeyCode::Escape => close(app),
        KeyCode::Backspace => erase(app),
        // Enter is a functional key, so SHIFT survives the kitty protocol
        // (unlike a shifted printable char, whose row would need the
        // shifted char itself with SHIFT cleared) — a genuinely distinct
        // `Enter + SHIFT` row, not a `CTRL|SHIFT` chord that could never
        // fire.
        KeyCode::Enter if key.mods == Mods::NONE => advance(app, true),
        KeyCode::Enter if key.mods == SHIFT => advance(app, false),
        // ↑/↓ browse history — filled in by a later change. Consumed here,
        // doing nothing, so it can't bleed into the document underneath in
        // the meantime.
        KeyCode::Up | KeyCode::Down => {}
        KeyCode::Char(c) if !c.is_control() && (key.mods == Mods::NONE || key.mods == SHIFT) => {
            type_char(app, c);
        }
        // Any other chord (an unbound Ctrl/Alt/Sup combo, or Enter with one)
        // is swallowed silently rather than typed or passed through — the
        // same discipline `title::keys::handle_key`'s own fallthrough uses.
        _ => {}
    }
    KeyOutcome::Consumed
}

/// Steps to the next (`forward`) or previous non-concealed match, wrapping
/// around the ends of the match list, from the active document's current
/// cursor position — the concealed check runs against the pre-jump
/// `concealed` cache ([`super::recompute`]'s own snapshot, not re-derived
/// here). A no-op when there are no matches, or every match is concealed.
/// Otherwise jumps the cursor onto the match, brings it on screen for a
/// read-only document (whose viewport never chases the cursor on its own —
/// `document::sync::scroll_to_cursor` short-circuits there), and persists
/// the query as just-used search history.
fn advance(app: &mut App, forward: bool) {
    let Some(state) = app.search.as_ref() else {
        return;
    };
    if state.matches.is_empty() {
        return;
    }
    let cursor_byte = app.active_doc().cursors.primary().position;
    let idx = if forward {
        next_index(&state.matches, cursor_byte, |r| {
            is_concealed(&state.concealed, r)
        })
    } else {
        prev_index(&state.matches, cursor_byte, |r| {
            is_concealed(&state.concealed, r)
        })
    };
    let Some(idx) = idx else {
        return;
    };
    let Some(range) = state.matches.get(idx).cloned() else {
        return;
    };
    let query = state.draft.clone();

    if let Some(s) = app.search.as_mut() {
        s.current = Some(idx);
    }
    app.active_doc_mut().cursors = CursorSet::new(range.start);
    if app.active_doc().is_read_only() {
        nav_scroll::scroll_to_byte_offset(app.active_doc_mut(), range.start);
    }
    persist_query(app, &query);
}

/// Records `query` as just-used in the recovery store's `search_history`
/// table and remembers it as the closed-bar navigation target
/// (`App::last_search_query`). A degraded or absent store enqueues
/// nothing; an enqueue `Err` is reported through the message pane and
/// otherwise ignored — a cosmetic history write must never sticky-degrade
/// the store the way a failed recovery write does.
fn persist_query(app: &mut App, query: &str) {
    app.last_search_query = Some(query.to_string());
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(db) = app.db.as_ref() else {
        return;
    };
    if let Err(e) = db.store.touch_search_query(query) {
        messages::error(app, format!("search history not saved: {e}"));
    }
}

fn type_char(app: &mut App, c: char) {
    if let Some(state) = app.search.as_mut() {
        state.draft.push(c);
    }
    recompute(app);
}

/// Erases one GRAPHEME CLUSTER, not one `char` (a combining mark popped
/// alone would desync what's on screen from what the buffer holds — the
/// same reasoning `explorer_search::handle_search`'s own `Erase` arm
/// applies). An already-empty draft has nothing to erase; the bar stays
/// open regardless (decision: only Esc closes it).
fn erase(app: &mut App) {
    if let Some(state) = app.search.as_mut()
        && let Some((byte_idx, _)) = state.draft.grapheme_indices(true).next_back()
    {
        state.draft.truncate(byte_idx);
    }
    recompute(app);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
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
    fn arrow_keys_are_consumed_stubs_that_touch_no_state() {
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
}
