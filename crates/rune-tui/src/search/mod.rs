//! In-file search: the pure match engine (case-insensitive matching over
//! the real buffer bytes, concealed-range collection so navigation can
//! skip a match hidden behind a substituted/decorated span, wraparound
//! stepping through a match list, and a fuzzy filter for browsing search
//! history), plus the bar's own state and the chokepoints that keep it in
//! sync with the active document. Keystroke handling, including ↑/↓ history
//! browsing, lives in the [`keys`] submodule; the bar's own row lives in
//! `render::search`, a sibling module, not a descendant — it reads
//! [`SearchState`] only through the fields this module marks `pub(crate)`.

use std::ops::Range;

use rune_syntax::wrap::WrapSnapshot;

use crate::app::App;
use crate::document::DocumentId;
use crate::runtime::{CmdError, Effects};

pub(crate) mod keys;

/// One in-file search bar's complete state — present on `App` only while
/// the bar is open (`App::search: Option<SearchState>`, decision: bar-open
/// IS `search.is_some()`, mirroring `messages::is_open`'s gate). The bar is
/// never a `Pane` (`focus.rs`'s recorded decision): `focused` is this
/// state's own bit, read by `focus::target` before it ever falls back to
/// the chrome-level `Pane`.
pub(crate) struct SearchState {
    pub(crate) focused: bool,
    pub(crate) draft: String,
    pub(crate) matches: Vec<Range<usize>>,
    pub(crate) current: Option<usize>,
    /// The document and buffer version `matches` was last computed
    /// against — [`sync`]'s own memo key, compared each frame so
    /// a tab switch or an edit that happens while the bar merely sits open
    /// (undo, an external reload) triggers exactly one recompute rather
    /// than paying for one on every idle frame.
    pub(crate) doc: DocumentId,
    pub(crate) buffer_version: u64,
    /// MRU search history, fuzzy-filterable — loaded once from
    /// `search_history` via a spawned `Cmd` when the bar opens
    /// ([`handle_history_loaded`]), browsed with ↑/↓ (`keys::history_prev`/
    /// `keys::history_next`).
    pub(crate) history: Vec<String>,
    /// The generation `Msg::SearchHistory`'s reply must echo back for
    /// [`handle_history_loaded`] to accept it — minted from
    /// `App::next_search_history_gen` at [`open`], the same shape
    /// `explorer_dirload`'s own request generation uses, and for the same
    /// reason: a close-then-reopen before a load lands must not have the
    /// stale reply land in the fresh session it wasn't answering.
    pub(crate) history_generation: crate::generation::Generation,
    /// The ↑/↓ browse cursor into the fuzzy-filtered history list — `None`
    /// while the draft itself (not a browsed entry) is what's showing, i.e.
    /// before the first ↑ or after ↓ has walked back past the newest entry.
    pub(crate) history_pos: Option<usize>,
    /// The draft as it stood the moment ↑ first started browsing — kept so
    /// every subsequent ↑/↓ filters against what the user actually TYPED,
    /// not against whatever history entry currently sits in `draft`, and so
    /// ↓ walking past the newest entry has the original in-progress draft
    /// to restore rather than losing it. `None` whenever `history_pos` is
    /// `None`; typing (`keys::type_char`/`erase`) clears both together.
    pub(crate) history_draft: Option<String>,
}

/// Opens the bar: a fresh, focused, empty draft. Never seeds from
/// `App::last_search_query` — the last query is for CLOSED-bar navigation
/// only (a later change), so re-opening the bar always starts blank. A
/// no-op if the bar is already open.
pub(crate) fn open(app: &mut App, effects: &mut Effects) {
    if app.search().is_some() {
        return;
    }
    let Some(clearance) = app.clear_title_for_overlay(effects) else {
        return;
    };
    let history_generation = app.next_search_history_gen.mint();
    app.open_search(
        SearchState {
            focused: true,
            draft: String::new(),
            matches: Vec::new(),
            current: None,
            doc: app.active,
            buffer_version: app.active_doc().buffer.version(),
            history: Vec::new(),
            history_generation,
            history_pos: None,
            history_draft: None,
        },
        clearance,
    );
}

/// Applies a `Msg::SearchHistory` reply: dropped outright
/// when the bar has since closed, or when `generation` no longer matches
/// the still-open bar's own `history_generation` — a close-then-reopen (or,
/// in principle, two overlapping loads) must never let a late reply for an
/// abandoned request land in the session that superseded it, mirroring
/// `explorer_dirload::handle_dir_loaded`'s own generation check. A reader
/// `Err` degrades `history` to empty rather than leaving whatever was there
/// (there's nothing durable to preserve — this is the FIRST load) and is
/// reported once through the message log; the bar itself keeps working
/// either way, since browsing an empty history is simply a no-op.
pub(crate) fn handle_history_loaded(
    app: &mut App,
    generation: crate::generation::Generation,
    result: Result<Vec<String>, CmdError>,
) {
    let current = app.search().map(|s| s.history_generation);
    if current != Some(generation) {
        return;
    }
    match result {
        Ok(entries) => {
            if let Some(state) = app.search_mut() {
                state.history = entries;
            }
        }
        Err(e) => {
            if let Some(state) = app.search_mut() {
                state.history = Vec::new();
            }
            crate::messages::error(app, format!("search history not loaded: {e}"));
        }
    }
}

/// The bar's one close chokepoint (`explorer_search::clear_search`
/// precedent): saves the live draft as `App::last_search_query` — so a
/// later change's closed-bar next/prev chord has something to navigate
/// with — then drops the state outright, which is what clears the
/// highlight overlay (it paints only while `App::search` is `Some`).
/// Leaves `App::focus` untouched: the bar was never a `Pane` to begin
/// with, so closing it can never "leave" a chrome region the way blurring
/// the title does.
pub(crate) fn close(app: &mut App) {
    let Some(state) = app.take_search() else {
        return;
    };
    if !state.draft.trim().is_empty() {
        app.last_search_query = Some(state.draft);
    }
}

/// The one chokepoint that keeps `matches` in step with the live draft,
/// active document, and buffer version (decision: "recompute over cache, no
/// shadow state") — called directly after every draft edit
/// (`keys::handle_key`) and, for a change that happens out from under an
/// already-open bar (a tab switch, an undo/redo, an external reload), from
/// [`sync`] below. Always resets `current` to `None`: a stale selected
/// index into a freshly recomputed match list could point at the wrong
/// match, or past the end of a now-shorter one. Concealment is NOT cached
/// here: it depends on reveal state (cursor) and viewport width, neither of
/// which bumps `buffer_version`, so [`concealed_ranges`] is instead
/// recomputed fresh at the point of use (`keys::jump`) from whatever `doc`
/// is active THEN, not whatever it was at this recompute.
pub(crate) fn recompute(app: &mut App) {
    if app.search().is_none() {
        return;
    }
    let draft = app.search().map(|s| s.draft.clone()).unwrap_or_default();
    let doc = app.active_doc();
    let matches = compute_matches(doc.buffer.content(), &draft);
    let version = doc.buffer.version();
    let doc_id = app.active;
    if let Some(state) = app.search_mut() {
        state.matches = matches;
        state.doc = doc_id;
        state.buffer_version = version;
        state.current = None;
    }
}

/// `App::sync_view`'s per-frame settle hook (called unconditionally; a
/// no-op with the bar closed): recomputes only when the active document or
/// its buffer version has drifted since the last recompute — every draft
/// edit already triggers [`recompute`] directly, so this is purely the
/// reactive half of the same chokepoint, catching a change that happens
/// underneath an already-open bar rather than through it.
pub(crate) fn sync(app: &mut App) {
    let Some(state) = app.search() else {
        return;
    };
    let stale =
        state.doc != app.active || state.buffer_version != app.active_doc().buffer.version();
    if stale {
        recompute(app);
    }
}

/// Folds `s` char-by-char via `char::to_lowercase`, returning the folded
/// string together with a per-folded-BYTE map back to the original char's
/// `(start, end)` buffer byte range. A single original char can fold to
/// several chars (e.g. 'İ' U+0130 folds to "i\u{0307}"), so the map has one
/// entry per folded byte, all pointing at the same original range — that
/// lets a folded-string match spanning part of an expansion snap outward to
/// the whole original char it came from.
fn fold_with_map(s: &str) -> (String, Vec<Range<usize>>) {
    let mut folded = String::new();
    let mut map = Vec::with_capacity(s.len());
    for (start, c) in s.char_indices() {
        let end = start + c.len_utf8();
        for lc in c.to_lowercase() {
            folded.push(lc);
            for _ in 0..lc.len_utf8() {
                map.push(start..end);
            }
        }
    }
    (folded, map)
}

/// Every case-insensitive occurrence of `query` in `haystack`, as byte
/// ranges into `haystack` itself (never into the folded intermediate).
/// Matching folds both sides with `char::to_lowercase` and maps hits back
/// through [`fold_with_map`] — a hit ending mid-expansion snaps outward to
/// whole original chars, so every returned range sits on real char
/// boundaries. An empty or whitespace-only query yields no matches (a plain
/// `match_indices("")` would otherwise return a hit at every position).
///
/// This folds with `char::to_lowercase`, not full Unicode case-folding: two
/// strings that only case-fold equal by the fuller algorithm (e.g. "SS" and
/// "ß") will not match each other here.
pub(crate) fn compute_matches(haystack: &str, query: &str) -> Vec<Range<usize>> {
    if query.trim().is_empty() {
        return Vec::new();
    }
    let (folded_hay, map) = fold_with_map(haystack);
    let folded_query: String = query.chars().flat_map(char::to_lowercase).collect();
    if folded_query.is_empty() {
        return Vec::new();
    }
    folded_hay
        .match_indices(&folded_query)
        .filter_map(|(s, matched)| {
            let e = s + matched.len();
            let start = map.get(s)?.start;
            let end = map.get(e - 1)?.end;
            Some(start..end)
        })
        .collect()
}

/// The buffer byte ranges currently concealed behind a substituted
/// (decorated) span — table borders, list bullets, and the like — collected
/// from every wrap segment's spans, sorted, and coalesced so a logical span
/// sliced across several wrapped rows reads back as one range. A `Substituted`
/// span's visible text differs from what's at its buffer range, which is
/// exactly what makes landing a cursor there produce no visible match to
/// look at; an `Identical` span's visible text is a verbatim buffer slice,
/// so it never contributes a concealed range even when folded content (e.g.
/// the "bold" inside `**bold**`) sits next to spans that do.
pub(crate) fn concealed_ranges(wrap: &WrapSnapshot) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = wrap
        .segments()
        .iter()
        .flat_map(|seg| seg.spans.iter())
        .filter(|span| span.is_rendered())
        .map(rune_syntax::SyntaxSpan::range)
        .collect();
    ranges.sort_by_key(|r| r.start);

    let mut coalesced: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match coalesced.last_mut() {
            Some(last) if r.start <= last.end => {
                if r.end > last.end {
                    last.end = r.end;
                }
            }
            _ => coalesced.push(r),
        }
    }
    coalesced
}

/// True iff `m` lies fully inside one of `ranges` (containment, not mere
/// overlap) — a match straddling a concealed edge still has visible text to
/// land on, so it is navigable; only a match wholly swallowed by a
/// concealed range is skipped. `ranges` is assumed sorted and coalesced, as
/// [`concealed_ranges`] returns it.
pub(crate) fn is_concealed(ranges: &[Range<usize>], m: &Range<usize>) -> bool {
    ranges.iter().any(|r| r.start <= m.start && m.end <= r.end)
}

/// The index into `matches` (assumed sorted ascending by `start`, as
/// [`compute_matches`] returns it) of the first non-skipped match strictly
/// after `cursor_byte`, wrapping around to the front when the cursor is
/// past every match. `None` when `matches` is empty or every match is
/// skipped.
pub(crate) fn next_index(
    matches: &[Range<usize>],
    cursor_byte: usize,
    skip: impl Fn(&Range<usize>) -> bool,
) -> Option<usize> {
    let n = matches.len();
    if n == 0 {
        return None;
    }
    let start = matches
        .iter()
        .position(|m| m.start > cursor_byte)
        .unwrap_or(0);
    (0..n)
        .map(|offset| (start + offset) % n)
        .find(|&idx| matches.get(idx).is_some_and(|m| !skip(m)))
}

/// The wraparound mirror of [`next_index`]: the first non-skipped match
/// strictly before `cursor_byte`, walking backward and wrapping to the end.
pub(crate) fn prev_index(
    matches: &[Range<usize>],
    cursor_byte: usize,
    skip: impl Fn(&Range<usize>) -> bool,
) -> Option<usize> {
    let n = matches.len();
    if n == 0 {
        return None;
    }
    let start = matches
        .iter()
        .rposition(|m| m.start < cursor_byte)
        .unwrap_or(n - 1);
    (0..n)
        .map(|offset| (start + n - offset) % n)
        .find(|&idx| matches.get(idx).is_some_and(|m| !skip(m)))
}

/// Case-insensitive subsequence match: every char of `needle` (lowercased)
/// must appear in `haystack` (lowercased) in order, not necessarily
/// adjacent.
fn is_subsequence(haystack: &str, needle: &[char]) -> bool {
    let mut chars = haystack.chars();
    needle.iter().all(|&nc| chars.any(|hc| hc == nc))
}

/// History entries whose text contains `draft` as a case-insensitive
/// subsequence, preserving `history`'s own (MRU-first) order. An empty
/// draft returns every entry unfiltered.
pub(crate) fn fuzzy_filter<'a>(history: &'a [String], draft: &str) -> Vec<&'a String> {
    if draft.is_empty() {
        return history.iter().collect();
    }
    let needle: Vec<char> = draft.to_lowercase().chars().collect();
    history
        .iter()
        .filter(|entry| is_subsequence(&entry.to_lowercase(), &needle))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
