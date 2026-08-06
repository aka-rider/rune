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
use crate::clipboard::pbpaste_cmd;
use crate::commands::nav_scroll;
use crate::keymap::{self, Command, KeyCode, KeyInput, KeyOutcome, Mods};
use crate::messages;
use crate::runtime::{Effects, PasteTarget};

use super::{close, is_concealed, next_index, prev_index, recompute};

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

pub(crate) fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    // ⌘V spawns the same `pbpaste` read every other paste target uses,
    // tagged `PasteTarget::Search` so the reply lands back in the draft
    // (`Msg::ClipboardRead`, `dispatch::update_inner`) rather than falling
    // through to `key.code`'s catch-all below, which would otherwise
    // swallow it silently.
    if keymap::resolve(key) == Some(Command::Paste) {
        effects.cmds.push(pbpaste_cmd(PasteTarget::Search));
        return KeyOutcome::Consumed;
    }
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

/// Appends pasted text to the draft — the search-bar counterpart of
/// `title::keys::paste`, reached from both `Msg::Paste` (bracketed paste
/// while the bar is focused) and `Msg::ClipboardRead { target: PasteTarget::
/// Search, .. }` (⌘V). Dropped outright once the bar has since closed: a
/// reply landing after Escape (or any focus-moving global that closes it —
/// `pane::handle_global_command`) has nowhere left to append to. Sanitized
/// the same way ordinary typing is (`type_char`'s own control-char guard),
/// first line only — the draft is rendered as a single row, so an embedded
/// newline would only ever show as a control glyph nobody typed.
pub(crate) fn paste(app: &mut App, text: &str) {
    if app.search.as_ref().is_none_or(|s| !s.focused) {
        return;
    }
    let sanitized: String = text
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    if sanitized.is_empty() {
        return;
    }
    if let Some(state) = app.search.as_mut() {
        state.draft.push_str(&sanitized);
        state.history_pos = None;
        state.history_draft = None;
    }
    recompute(app);
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
mod tests;
