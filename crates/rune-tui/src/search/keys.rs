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
        KeyCode::Up => history_prev(app),
        KeyCode::Down => history_next(app),
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
///
/// Also reached with the bar OPEN from the closed-bar next/prev chords
/// (`GlobalCommand::SearchNext`/`SearchPrev`, `pane::handle_global_command`)
/// — identical behavior to Enter/Shift+Enter in that state, per plan WP5.
pub(crate) fn advance(app: &mut App, forward: bool) {
    let Some(state) = app.search.as_ref() else {
        return;
    };
    let matches = state.matches.clone();
    let concealed = state.concealed.clone();
    let query = state.draft.clone();
    let Some(idx) = jump(app, &matches, &concealed, forward) else {
        return;
    };
    if let Some(s) = app.search.as_mut() {
        s.current = Some(idx);
    }
    persist_query(app, &query);
}

/// The closed-bar mirror of [`advance`] (`GlobalCommand::SearchNext`/
/// `SearchPrev`, plan WP5.S1): there is no `SearchState` to read `matches`/
/// `concealed` from — the bar isn't open — so both are recomputed on demand
/// from `App::last_search_query` via the same pure functions
/// [`super::recompute`] itself calls, then jumped through the identical
/// [`jump`] helper `advance` uses (same concealed-skip, same read-only
/// scroll). Paints no highlights: those exist only while the bar is open
/// (decision A2). Returns `false` without doing anything when there is no
/// last query to navigate with — the caller is responsible for surfacing
/// that with user-visible feedback, since a silently swallowed chord is
/// never acceptable.
pub(crate) fn advance_closed(app: &mut App, forward: bool) -> bool {
    let Some(query) = app.last_search_query.clone() else {
        return false;
    };
    let matches = super::compute_matches(app.active_doc().buffer.content(), &query);
    let concealed = app
        .active_doc()
        .view
        .as_ref()
        .map(|view| super::concealed_ranges(&view.wrap))
        .unwrap_or_default();
    if jump(app, &matches, &concealed, forward).is_some() {
        persist_query(app, &query);
    }
    true
}

/// The shared cursor-jump core [`advance`] and [`advance_closed`] both
/// funnel through: given a match list and its concealed cache (in whatever
/// coordinate space the caller sourced them from — live `SearchState` or a
/// closed-bar on-demand recompute), finds the next/prev non-concealed match
/// relative to the active document's cursor, moves the cursor there and,
/// for a read-only document, the viewport too. Returns the index into
/// `matches` on success; `None` when there's nothing to land on (empty
/// list, or every match concealed) — the caller decides what "nothing
/// happened" means for its own state.
fn jump(
    app: &mut App,
    matches: &[std::ops::Range<usize>],
    concealed: &[std::ops::Range<usize>],
    forward: bool,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let cursor_byte = app.active_doc().cursors.primary().position;
    let idx = if forward {
        next_index(matches, cursor_byte, |r| is_concealed(concealed, r))
    } else {
        prev_index(matches, cursor_byte, |r| is_concealed(concealed, r))
    };
    let idx = idx?;
    let range = matches.get(idx)?.clone();
    app.active_doc_mut().cursors = CursorSet::new(range.start);
    if app.active_doc().is_read_only() {
        nav_scroll::scroll_to_byte_offset(app.active_doc_mut(), range.start);
    }
    Some(idx)
}

/// Records `query` as just-used in the recovery store's `search_history`
/// table and remembers it as the closed-bar navigation target
/// (`App::last_search_query`). A degraded or absent store enqueues
/// nothing. The enqueued op id is tracked in `App::search_history_ops` so
/// `db_dispatch::handle_db_event` can recognize a LATER `DbEvent::Err` for
/// this exact write as a cosmetic failure rather than a real recovery one —
/// an immediate enqueue `Err` (this write never even reached the writer's
/// queue) has no such op id to track, so it's reported right here instead.
/// Either way: a message through the log, never `on_store_failure`'s sticky
/// degrade — a failed history touch must not disable recovery for the rest
/// of the session.
fn persist_query(app: &mut App, query: &str) {
    app.last_search_query = Some(query.to_string());
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some(db) = app.db.as_ref() else {
        return;
    };
    match db.store.touch_search_query(query) {
        Ok(op_id) => {
            app.search_history_ops.insert(op_id);
        }
        Err(e) => {
            messages::error(app, format!("search history not saved: {e}"));
        }
    }
}

fn type_char(app: &mut App, c: char) {
    if let Some(state) = app.search.as_mut() {
        state.draft.push(c);
        state.history_pos = None;
        state.history_draft = None;
    }
    recompute(app);
}

/// Erases one GRAPHEME CLUSTER, not one `char` (a combining mark popped
/// alone would desync what's on screen from what the buffer holds — the
/// same reasoning `explorer_search::handle_search`'s own `Erase` arm
/// applies). An already-empty draft has nothing to erase; the bar stays
/// open regardless (decision: only Esc closes it).
fn erase(app: &mut App) {
    if let Some(state) = app.search.as_mut() {
        state.history_pos = None;
        state.history_draft = None;
        if let Some((byte_idx, _)) = state.draft.grapheme_indices(true).next_back() {
            state.draft.truncate(byte_idx);
        }
    }
    recompute(app);
}

/// The needle every ↑/↓ browse step filters the history against: the draft
/// as it stood the moment browsing started (`SearchState::history_draft`),
/// or — before the first ↑ this bar-open has seen — the live draft itself.
fn browse_needle(state: &super::SearchState) -> String {
    state
        .history_draft
        .clone()
        .unwrap_or_else(|| state.draft.clone())
}

/// ↑: steps one entry OLDER in the fuzzy-filtered MRU history, clamping at
/// the oldest match rather than wrapping — history browsing is a one-way
/// walk back through time, not a ring like match navigation. The first ↑ of
/// a bar-open session captures the live draft into `history_draft` before
/// replacing `draft` with the selected entry, so every subsequent step (and
/// [`history_next`]'s own restore) still filters against what the user
/// actually typed. A no-op with no history, or when nothing in it matches
/// the needle.
fn history_prev(app: &mut App) {
    let Some(state) = app.search.as_ref() else {
        return;
    };
    if state.history.is_empty() {
        return;
    }
    let needle = browse_needle(state);
    let filtered: Vec<String> = super::fuzzy_filter(&state.history, &needle)
        .into_iter()
        .cloned()
        .collect();
    if filtered.is_empty() {
        return;
    }
    let next_pos = state
        .history_pos
        .map_or(0, |pos| (pos + 1).min(filtered.len() - 1));
    let Some(entry) = filtered.get(next_pos).cloned() else {
        return;
    };

    let Some(state) = app.search.as_mut() else {
        return;
    };
    state.history_draft.get_or_insert(needle);
    state.history_pos = Some(next_pos);
    state.draft = entry;
    recompute(app);
}

/// ↓: the mirror of [`history_prev`], stepping one entry NEWER. Walking
/// past the newest filtered entry restores the pre-browse draft captured by
/// the first ↑ and ends the browse session (`history_pos`/`history_draft`
/// both clear) — the in-progress draft the user was typing before pressing
/// ↑ is never lost. A no-op while no browse session is active
/// (`history_pos` is `None`).
fn history_next(app: &mut App) {
    let Some(state) = app.search.as_ref() else {
        return;
    };
    let Some(pos) = state.history_pos else {
        return;
    };
    if pos == 0 {
        let restored = state.history_draft.clone().unwrap_or_default();
        let Some(state) = app.search.as_mut() else {
            return;
        };
        state.draft = restored;
        state.history_pos = None;
        state.history_draft = None;
        recompute(app);
        return;
    }
    let needle = browse_needle(state);
    let filtered: Vec<String> = super::fuzzy_filter(&state.history, &needle)
        .into_iter()
        .cloned()
        .collect();
    let next_pos = pos - 1;
    let Some(entry) = filtered.get(next_pos).cloned() else {
        return;
    };

    let Some(state) = app.search.as_mut() else {
        return;
    };
    state.history_pos = Some(next_pos);
    state.draft = entry;
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
}
