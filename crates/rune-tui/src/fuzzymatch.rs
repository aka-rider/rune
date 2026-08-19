//! The nucleo scoring primitive the palette and file finder rankers both
//! call once per candidate field — score against the live `Pattern`, and,
//! only for a row that survives ranking, the sorted-deduped match indices
//! render bolds. Each overlay keeps its own long-lived `Matcher`/`charbuf`
//! (`PaletteState`/`FileSearchState`): the two can never be open at once
//! (`Overlay` is a single slot), but neither state outlives the overlay
//! session, so there is no shared instance worth hoisting them into.

use nucleo_matcher::pattern::Pattern;
use nucleo_matcher::{Matcher, Utf32Str};

pub(crate) fn score(
    text: &str,
    pattern: &Pattern,
    matcher: &mut Matcher,
    charbuf: &mut Vec<char>,
) -> Option<u32> {
    pattern.score(Utf32Str::new(text, charbuf), matcher)
}

/// The matched-grapheme indices for `text`, sorted and deduped — `Pattern::
/// indices` never clears its own output vec and a multi-atom pattern
/// appends per atom, not pre-sorted.
pub(crate) fn indices(
    text: &str,
    pattern: &Pattern,
    matcher: &mut Matcher,
    charbuf: &mut Vec<char>,
) -> Vec<u32> {
    let mut indices = Vec::new();
    let _ = pattern.indices(Utf32Str::new(text, charbuf), matcher, &mut indices);
    indices.sort_unstable();
    indices.dedup();
    indices
}
