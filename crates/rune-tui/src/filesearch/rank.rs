//! Non-empty-query ranking for the fuzzy file finder (plan WP4): scores
//! every candidate against the live query with the session's own
//! long-lived `Matcher`, partitions in-tree above out-of-tree, and orders
//! each partition by score, then MRU rank, then display width, then
//! alphabetically — all of it here, inside `update`'s own call chain,
//! never in render. Highlight indices are computed in a second pass, over
//! only the rows that survive the cap, since a row nothing ever displays
//! needs none.

use nucleo_matcher::Utf32Str;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};

use super::{Candidate, FileSearchState, RESULT_CAP, ResultRow, candidate_by};

/// One scored candidate, kept only long enough to sort and cap — the
/// display string is cloned here (not re-borrowed) so the sort comparator
/// doesn't have to juggle a second borrow of `state.recents`/`state.walk`
/// alongside the `&mut Matcher`/`&mut Vec<char>` scoring already needs.
struct Scored {
    candidate_idx: usize,
    score: u32,
    in_tree: bool,
    mru_rank: Option<usize>,
    width: usize,
    display: String,
}

/// Replaces `state.results` with the live query's fuzzy-ranked matches.
pub(super) fn rank(state: &mut FileSearchState) {
    let pattern = Pattern::parse(&state.query, CaseMatching::Smart, Normalization::Smart);
    let FileSearchState {
        recents,
        walk,
        matcher,
        charbuf,
        results,
        ..
    } = state;

    let mut scored = score_all(recents, 0, &pattern, matcher, charbuf);
    scored.extend(score_all(walk, recents.len(), &pattern, matcher, charbuf));
    scored.sort_by(|a, b| {
        b.in_tree
            .cmp(&a.in_tree)
            .then_with(|| b.score.cmp(&a.score))
            .then_with(|| mru_key(a.mru_rank).cmp(&mru_key(b.mru_rank)))
            .then_with(|| a.width.cmp(&b.width))
            .then_with(|| a.display.cmp(&b.display))
    });
    scored.truncate(RESULT_CAP);

    *results = scored
        .into_iter()
        .map(|s| ResultRow {
            indices: indices_for(s.candidate_idx, recents, walk, &pattern, matcher, charbuf),
            candidate_idx: s.candidate_idx,
        })
        .collect();
}

fn score_all(
    candidates: &[Candidate],
    offset: usize,
    pattern: &Pattern,
    matcher: &mut nucleo_matcher::Matcher,
    charbuf: &mut Vec<char>,
) -> Vec<Scored> {
    candidates
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let score = pattern.score(Utf32Str::new(&c.display, charbuf), matcher)?;
            Some(Scored {
                candidate_idx: offset + i,
                score,
                in_tree: c.in_tree,
                mru_rank: c.mru_rank,
                width: crate::width::display_width(&c.display),
                display: c.display.clone(),
            })
        })
        .collect()
}

/// The matched-grapheme indices `render::filesearch` bolds, for the one
/// candidate `idx` names. `Pattern::indices` never clears its own output
/// vec, so this always clears first, then sorts and dedups the raw
/// per-atom indices it appends — `Pattern::indices`'s own doc: multi-atom
/// output is appended per atom, not pre-sorted or deduped.
fn indices_for(
    idx: usize,
    recents: &[Candidate],
    walk: &[Candidate],
    pattern: &Pattern,
    matcher: &mut nucleo_matcher::Matcher,
    charbuf: &mut Vec<char>,
) -> Vec<u32> {
    let mut indices = Vec::new();
    if let Some(c) = candidate_by(recents, walk, idx) {
        indices.clear();
        let _ = pattern.indices(Utf32Str::new(&c.display, charbuf), matcher, &mut indices);
        indices.sort_unstable();
        indices.dedup();
    }
    indices
}

/// `Some` ranks before `None`, ascending within `Some` — `Option<usize>`'s
/// own `Ord` puts `None` first, the opposite of what MRU tie-breaking
/// needs, so this remaps `None` to a sentinel past every real rank instead.
fn mru_key(rank: Option<usize>) -> usize {
    rank.unwrap_or(usize::MAX)
}
