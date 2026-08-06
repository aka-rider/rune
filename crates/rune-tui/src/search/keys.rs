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

use crate::app::App;
use crate::keymap::{KeyCode, KeyInput, KeyOutcome, Mods};
use crate::runtime::Effects;

use super::{close, recompute};

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
        // Enter/Shift+Enter navigate the match set; ↑/↓ browse history —
        // both filled in by a later change. Consumed here, doing nothing,
        // so neither can bleed into the document underneath in the
        // meantime.
        KeyCode::Enter | KeyCode::Up | KeyCode::Down => {}
        KeyCode::Char(c) if !c.is_control() && (key.mods == Mods::NONE || key.mods == SHIFT) => {
            type_char(app, c);
        }
        // Any other chord (an unbound Ctrl/Alt/Sup combo) is swallowed
        // silently rather than typed or passed through — the same
        // discipline `title::keys::handle_key`'s own fallthrough uses.
        _ => {}
    }
    KeyOutcome::Consumed
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
    fn enter_and_arrow_keys_are_consumed_stubs_that_touch_no_state() {
        let mut app = app_with("hello");
        crate::search::open(&mut app);
        let mut effects = Effects::default();
        let _ = handle_key(&mut app, char_key('h'), &mut effects);
        let before = app.search.as_ref().unwrap().draft.clone();

        for code in [KeyCode::Enter, KeyCode::Up, KeyCode::Down] {
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
}
