use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};

use crate::fuzzymatch;

use super::{Candidate, FileSearchState, RESULT_CAP, ResultRow, candidate_by};

struct Scored {
    candidate_idx: usize,
    score: u32,
    in_tree: bool,
    mru_rank: Option<usize>,
    width: usize,
}

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

fn indices_for(
    idx: usize,
    recents: &[Candidate],
    walk: &[Candidate],
    pattern: &Pattern,
    matcher: &mut nucleo_matcher::Matcher,
    charbuf: &mut Vec<char>,
) -> Vec<u32> {
    // `Pattern::indices` appends per-atom output without sorting or
    // deduping it, so multi-atom results are sorted/deduped here.
    candidate_by(recents, walk, idx)
        .map(|c| fuzzymatch::indices(&c.display, pattern, matcher, charbuf))
        .unwrap_or_default()
}

fn display_of<'a>(recents: &'a [Candidate], walk: &'a [Candidate], idx: usize) -> &'a str {
    candidate_by(recents, walk, idx).map_or("", |c| c.display.as_str())
}

fn mru_key(rank: Option<usize>) -> usize {
    rank.unwrap_or(usize::MAX)
}
