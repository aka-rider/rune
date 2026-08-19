//! Keystroke handling for the focused search bar — reached from
//! `dispatch::handle_key`'s stage 3 whenever `focus::target` resolves to
//! `FocusTarget::SearchField`, ahead of the ordinary chrome-level `Pane`
//! match, since the bar is never itself a `Pane`.
//!
//! Every path returns [`KeyOutcome::Consumed`] — Title's own discipline
//! (`title/keys.rs`), required by the fuzzer's `PANE-NO-BLEED` invariant:
//! a keystroke aimed at the bar must never fall through and mutate the
//! document buffer underneath it.

use std::ops::Range;

use rune_core::cursor::CursorSet;

use crate::app::App;
use crate::clipboard::pbpaste_cmd;
use crate::keymap::{self, Command, KeyCode, KeyInput, KeyOutcome, Mods};
use crate::messages;
use crate::queryline;
use crate::runtime::{Effects, PasteTarget};
use crate::viewport::ScrollMode;

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
    if app.search().is_none_or(|s| !s.focused) {
        return;
    }
    let sanitized = queryline::sanitize_pasted_line(text);
    if sanitized.is_empty() {
        return;
    }
    if let Some(state) = app.search_mut() {
        state.draft.push_str(&sanitized);
        state.history_pos = None;
        state.history_draft = None;
    }
    recompute(app);
}

/// Steps to the next (`forward`) or previous non-concealed match, wrapping
/// around the ends of the match list, from the active document's current
/// cursor position. Revalidates against the live document FIRST: the
/// runtime drains a whole batch of messages before the next `sync_view`
/// (`super::sync`'s only caller), so an async doc-switch coalesced with this
/// very Enter in one batch could otherwise leave `state.matches` holding
/// byte ranges from the document that was active a moment ago — a stale
/// `doc`/`buffer_version` triggers one [`recompute`] right here before
/// anything is read. The concealed set is never cached: it depends on
/// reveal state and viewport width, not `buffer_version`, so it is
/// recomputed fresh from the CURRENT view on every call. Every outcome is
/// reported: a jump moves the cursor and persists the query as search
/// history; nothing navigable posts a distinct message explaining why
/// (`no matches`/`all N matches are concealed`) rather than leaving the
/// keypress silent.
///
/// Also reached with the bar OPEN from the closed-bar next/prev chords
/// (`GlobalCommand::SearchNext`/`SearchPrev`, `pane::handle_global_command`)
/// — identical behavior to Enter/Shift+Enter in that state.
pub(crate) fn advance(app: &mut App, forward: bool) {
    let Some(state) = app.search() else {
        return;
    };
    if state.doc != app.active || state.buffer_version != app.active_doc().buffer.version() {
        recompute(app);
    }
    let Some(state) = app.search() else {
        return;
    };
    let matches = state.matches.clone();
    let query = state.draft.clone();
    let concealed = current_concealed(app);
    match jump(app, &matches, &concealed, forward) {
        Some(idx) => {
            if let Some(s) = app.search_mut() {
                s.current = Some(idx);
            }
            persist_query(app, &query);
        }
        None => report_no_target(app, &query, &matches),
    }
}

/// The closed-bar mirror of [`advance`] (`GlobalCommand::SearchNext`/
/// `SearchPrev`): there is no `SearchState` to read `matches`
/// from — the bar isn't open — so it's recomputed on demand from
/// `App::last_search_query` via the same pure function [`super::recompute`]
/// itself calls, the concealed set freshly from the current view exactly as
/// [`advance`] does, then jumped through the identical [`jump`] helper
/// `advance` uses (same concealed-skip, same read-only scroll). Paints no
/// highlights: those exist only while the bar is open (decision A2).
/// Returns `false` without doing anything when there is no last query at
/// all — the caller reports that case (`pane::search_step`'s "no previous
/// search"). A last query that simply has no navigable target in THIS
/// document is instead reported right here, same as [`advance`], so the
/// caller's silence never masks it.
pub(crate) fn advance_closed(app: &mut App, forward: bool) -> bool {
    let Some(query) = app.last_search_query.clone() else {
        return false;
    };
    let matches = super::compute_matches(app.active_doc().buffer.content(), &query);
    let concealed = current_concealed(app);
    match jump(app, &matches, &concealed, forward) {
        Some(_) => persist_query(app, &query),
        None => report_no_target(app, &query, &matches),
    }
    true
}

/// The concealed byte ranges as of THIS instant's active document view —
/// never cached, since concealment tracks reveal state (cursor) and
/// viewport width, neither of which bumps `buffer_version`; a mouse click
/// that reveals a table row must make its matches navigable on the very
/// next Enter, not just after the next edit.
fn current_concealed(app: &App) -> Vec<Range<usize>> {
    app.active_doc()
        .view
        .as_ref()
        .map(|view| super::concealed_ranges(&view.wrap))
        .unwrap_or_default()
}

/// Posts the feedback [`advance`]/[`advance_closed`] owe the user when
/// [`jump`] found nothing to land on: a blank query is left silent (nothing
/// was searched for), an empty match list names the query, and a non-empty
/// list whose every match is concealed says so distinctly, since "no
/// matches" would be simply false in that case.
fn report_no_target(app: &mut App, query: &str, matches: &[Range<usize>]) {
    if matches.is_empty() {
        if !query.trim().is_empty() {
            messages::info(app, format!("no matches for \"{query}\""));
        }
    } else {
        messages::info(app, format!("all {} matches are concealed", matches.len()));
    }
}

/// The shared cursor-jump core [`advance`] and [`advance_closed`] both
/// funnel through: given a match list and the concealed set (freshly
/// computed by the caller — live bar via [`current_concealed`] or a
/// closed-bar on-demand recompute), finds the next/prev non-concealed match
/// relative to the active document's cursor, moves the cursor there and,
/// for a read-only document, the viewport too. Returns the index into
/// `matches` on success; `None` when there's nothing to land on (empty
/// list, or every match concealed) — the caller decides what "nothing
/// happened" means for its own state.
fn jump(
    app: &mut App,
    matches: &[Range<usize>],
    concealed: &[Range<usize>],
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
    let doc = app.active_doc_mut();
    doc.cursors = CursorSet::new(range.start);
    doc.viewport.mode = ScrollMode::EnsureVisible;
    Some(idx)
}

/// Records `query` as just-used in the recovery store's `search_history`
/// table and remembers it as the closed-bar navigation target
/// (`App::last_search_query`). Debounced by `App::search_history`
/// (`HistoryPersistence`) against the last value it persisted: wrapping
/// back onto the same match (or simply pressing Enter again on an
/// unchanged query) enqueues nothing after the first time — a DB write per
/// key-repeat would be pure waste. A degraded or absent store enqueues
/// nothing. The enqueued op id is tracked so `db_dispatch::handle_db_event` can
/// recognize a LATER `DbEvent::Err` for this exact write as a cosmetic
/// failure rather than a real recovery one — an immediate enqueue `Err`
/// (this write never even reached the writer's queue) has no such op id to
/// track, so it's reported right here instead. Either way: a message
/// through the log, never `on_store_failure`'s sticky degrade — a failed
/// history touch must not disable recovery for the rest of the session.
fn persist_query(app: &mut App, query: &str) {
    app.last_search_query = Some(query.to_string());
    let result = app.search_history.touch(app.db.as_ref(), query, |db| {
        db.store.touch_search_query(query)
    });
    if let Some(Err(e)) = result {
        messages::error(app, format!("search history not saved: {e}"));
    }
}

fn type_char(app: &mut App, c: char) {
    if let Some(state) = app.search_mut() {
        queryline::type_char(&mut state.draft, c);
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
    if let Some(state) = app.search_mut() {
        state.history_pos = None;
        state.history_draft = None;
        queryline::erase_grapheme(&mut state.draft);
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

/// Which way [`history_step`] walks the fuzzy-filtered MRU history.
#[derive(Clone, Copy)]
enum BrowseDir {
    /// ↑: one entry OLDER, clamping at the oldest match rather than
    /// wrapping — history browsing is a one-way walk back through time, not
    /// a ring like match navigation.
    Prev,
    /// ↓: one entry NEWER; walking past the newest filtered entry ends the
    /// browse session instead of clamping (see [`history_step`]).
    Next,
}

/// The shared body of [`history_prev`]/[`history_next`]: fetches the
/// fuzzy-filtered history, steps `history_pos` one entry in `dir`'s
/// direction, and writes the selected entry into `draft` — differing from
/// its mirror only in which way `history_pos` moves and, for `Next`,
/// walking off the near end (`history_pos == Some(0)`) restores the
/// pre-browse draft captured by the first ↑ (`history_draft`) and ends the
/// session entirely rather than clamping. The first ↑ of a bar-open session
/// captures the live draft into `history_draft` before replacing `draft`
/// with the selected entry, so every subsequent step still filters against
/// what the user actually typed. A no-op with no history, or when nothing
/// in it matches the needle; for `Next`, also a no-op while no browse
/// session is active (`history_pos` is `None`).
fn history_step(app: &mut App, dir: BrowseDir) {
    let Some(state) = app.search() else {
        return;
    };
    if let BrowseDir::Next = dir {
        let Some(pos) = state.history_pos else {
            return;
        };
        if pos == 0 {
            let restored = state.history_draft.clone().unwrap_or_default();
            let Some(state) = app.search_mut() else {
                return;
            };
            state.draft = restored;
            state.history_pos = None;
            state.history_draft = None;
            recompute(app);
            return;
        }
    }

    let needle = browse_needle(state);
    let filtered: Vec<String> = super::fuzzy_filter(&state.history, &needle)
        .into_iter()
        .cloned()
        .collect();
    let next_pos = match dir {
        BrowseDir::Prev => state
            .history_pos
            .map_or(0, |pos| (pos + 1).min(filtered.len().saturating_sub(1))),
        BrowseDir::Next => state.history_pos.map_or(0, |pos| pos - 1),
    };
    let Some(entry) = filtered.get(next_pos).cloned() else {
        return;
    };

    let Some(state) = app.search_mut() else {
        return;
    };
    if let BrowseDir::Prev = dir {
        state.history_draft.get_or_insert(needle);
    }
    state.history_pos = Some(next_pos);
    state.draft = entry;
    recompute(app);
}

fn history_prev(app: &mut App) {
    history_step(app, BrowseDir::Prev);
}

fn history_next(app: &mut App) {
    history_step(app, BrowseDir::Next);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod editing_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod history_tests;
