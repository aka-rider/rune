//! Non-empty-query ranking for the fuzzy file finder: scores
//! every candidate against the live query with the session's own
//! long-lived `Matcher`, partitions in-tree above out-of-tree, and orders
//! each partition by score, then MRU rank, then display width, then
//! alphabetically — all of it here, inside `update`'s own call chain,
//! never in render. Highlight indices are computed in a second pass, over
//! only the rows that survive the cap, since a row nothing ever displays
//! needs none.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};

use crate::fuzzymatch;

use super::{Candidate, FileSearchState, RESULT_CAP, ResultRow, candidate_by};

/// One scored candidate, kept only long enough to sort and cap — the
/// display string is resolved back through `candidate_by` when the sort
/// comparator needs it, rather than cloned into this struct up front.
struct Scored {
    candidate_idx: usize,
    score: u32,
    in_tree: bool,
    mru_rank: Option<usize>,
    width: usize,
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
            .then_with(|| {
                display_of(recents, walk, a.candidate_idx).cmp(display_of(
                    recents,
                    walk,
                    b.candidate_idx,
                ))
            })
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
            let score = fuzzymatch::score(&c.display, pattern, matcher, charbuf)?;
            Some(Scored {
                candidate_idx: offset + i,
                score,
                in_tree: c.in_tree,
                mru_rank: c.mru_rank,
                width: crate::width::display_width(&c.display),
            })
        })
        .collect()
}

/// The matched-grapheme indices `render::filesearch` bolds, for the one
/// candidate `idx` names. `Pattern::indices` never clears its own output
/// vec, so the raw per-atom indices it appends are sorted and deduped here
/// — `Pattern::indices`'s own doc: multi-atom output is appended per atom,
/// not pre-sorted or deduped.
fn indices_for(
    idx: usize,
    recents: &[Candidate],
    walk: &[Candidate],
    pattern: &Pattern,
    matcher: &mut nucleo_matcher::Matcher,
    charbuf: &mut Vec<char>,
) -> Vec<u32> {
    candidate_by(recents, walk, idx)
        .map(|c| fuzzymatch::indices(&c.display, pattern, matcher, charbuf))
        .unwrap_or_default()
}

fn display_of<'a>(recents: &'a [Candidate], walk: &'a [Candidate], idx: usize) -> &'a str {
    candidate_by(recents, walk, idx).map_or("", |c| c.display.as_str())
}

/// `Some` ranks before `None`, ascending within `Some` — `Option<usize>`'s
/// own `Ord` puts `None` first, the opposite of what MRU tie-breaking
/// needs, so this remaps `None` to a sentinel past every real rank instead.
fn mru_key(rank: Option<usize>) -> usize {
    rank.unwrap_or(usize::MAX)
}
