//! In-file search: the pure match engine (case-insensitive matching over
//! the real buffer bytes, concealed-range collection so navigation can
//! skip a match hidden behind a substituted/decorated span, wraparound
//! stepping through a match list, and a fuzzy filter for browsing search
//! history), plus the bar's own state and the chokepoints that keep it in
//! sync with the active document. Keystroke handling lives in the [`keys`]
//! submodule; the bar's own row lives in `render::search`, a sibling
//! module, not a descendant — it reads [`SearchState`] only through the
//! fields this module marks `pub(crate)`.
//!
//! `next_index`/`prev_index`/`fuzzy_filter` have no production caller yet:
//! wraparound navigation and history browsing land in a later change. Each
//! is individually `#[allow(dead_code)]`'d rather than the whole file, so a
//! function that gains a real caller loses its allow and the lint keeps
//! watching the rest.

use std::ops::Range;

use rune_syntax::wrap::WrapSnapshot;

use crate::app::App;
use crate::document::DocumentId;

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
    /// The document and buffer version `matches`/`concealed` were last
    /// computed against — [`sync`]'s own memo key, compared each frame so
    /// a tab switch or an edit that happens while the bar merely sits open
    /// (undo, an external reload) triggers exactly one recompute rather
    /// than paying for one on every idle frame.
    doc: DocumentId,
    buffer_version: u64,
    /// The concealed byte ranges cached alongside `matches` at the same
    /// [`recompute`] — [`concealed_ranges`]'s own output. Unread within
    /// this change: the navigation that consults it (skipping a match
    /// wholly hidden behind a substituted span) lands in a later change,
    /// which is exactly why it is computed and cached HERE rather than
    /// re-derived at that later call site — one recompute chokepoint, not
    /// two.
    #[allow(dead_code)]
    concealed: Vec<Range<usize>>,
    /// MRU search history, fuzzy-filterable — populated by a later change
    /// (loaded from `search_history` once the bar opens).
    #[allow(dead_code)]
    history: Vec<String>,
    /// The generation `Msg::SearchHistory`'s reply must echo back for a
    /// later change's async history load to accept it — minted fresh each
    /// bar-open, same shape as `explorer_dirload`'s own request generation.
    #[allow(dead_code)]
    history_generation: u64,
    /// The ↑/↓ browse cursor into the fuzzy-filtered history list — `None`
    /// while the draft itself (not a browsed entry) is what's showing.
    /// Written by a later change.
    #[allow(dead_code)]
    history_pos: Option<usize>,
}

/// Opens the bar: a fresh, focused, empty draft. Never seeds from
/// `App::last_search_query` — the last query is for CLOSED-bar navigation
/// only (a later change), so re-opening the bar always starts blank. A
/// no-op if the bar is already open.
pub(crate) fn open(app: &mut App) {
    if app.search.is_some() {
        return;
    }
    app.search = Some(SearchState {
        focused: true,
        draft: String::new(),
        matches: Vec::new(),
        current: None,
        doc: app.active,
        buffer_version: app.active_doc().buffer.version(),
        concealed: Vec::new(),
        history: Vec::new(),
        history_generation: 0,
        history_pos: None,
    });
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
    let Some(state) = app.search.take() else {
        return;
    };
    if !state.draft.trim().is_empty() {
        app.last_search_query = Some(state.draft);
    }
}

/// The one chokepoint that keeps `matches`/`concealed` in step with the
/// live draft, active document, and buffer version (decision: "recompute
/// over cache, no shadow state") — called directly after every draft edit
/// (`keys::handle_key`) and, for a change that happens out from under an
/// already-open bar (a tab switch, an undo/redo, an external reload), from
/// [`sync`] below. Always resets `current` to `None`: a stale selected
/// index into a freshly recomputed match list could point at the wrong
/// match, or past the end of a now-shorter one.
pub(crate) fn recompute(app: &mut App) {
    if app.search.is_none() {
        return;
    }
    let draft = app
        .search
        .as_ref()
        .map(|s| s.draft.clone())
        .unwrap_or_default();
    let doc = app.active_doc();
    let matches = compute_matches(doc.buffer.content(), &draft);
    let concealed = doc
        .view
        .as_ref()
        .map(|view| concealed_ranges(&view.wrap))
        .unwrap_or_default();
    let version = doc.buffer.version();
    let doc_id = app.active;
    if let Some(state) = app.search.as_mut() {
        state.matches = matches;
        state.concealed = concealed;
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
    let Some(state) = app.search.as_ref() else {
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
            map.push(start..end);
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
        .map(|span| span.range())
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
///
/// No production caller yet — the navigation that consults this to skip a
/// concealed match lands in a later change.
#[allow(dead_code)]
pub(crate) fn is_concealed(ranges: &[Range<usize>], m: &Range<usize>) -> bool {
    ranges.iter().any(|r| r.start <= m.start && m.end <= r.end)
}

/// The index into `matches` (assumed sorted ascending by `start`, as
/// [`compute_matches`] returns it) of the first non-skipped match strictly
/// after `cursor_byte`, wrapping around to the front when the cursor is
/// past every match. `None` when `matches` is empty or every match is
/// skipped.
///
/// No production caller yet — wraparound Enter/Shift+Enter navigation
/// lands in a later change.
#[allow(dead_code)]
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
///
/// No production caller yet — see [`next_index`]'s doc.
#[allow(dead_code)]
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
///
/// No production caller yet — history browsing lands in a later change.
#[allow(dead_code)]
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
