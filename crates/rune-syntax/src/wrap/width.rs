//! The wrap pass's grapheme/width chokepoint (CONSTITUTION §1.6 split of
//! the wrap module): every place that measures a rune's or a whole
//! grapheme cluster's DISPLAY width, and the one place every width walker
//! draws its cluster boundaries from. Shared, per the docs on each item
//! below, with `rune-tui`'s renderer and this module's sibling `query`
//! submodule — none of them may re-derive these numbers independently.

use unicode_segmentation::UnicodeSegmentation;

/// Halfwidth Katakana Voiced Sound Mark (dakuten) — `unicode-width` reports
/// this as zero-width (it carries `Grapheme_Extend`), but terminals render
/// it as its own halfwidth cell. Mirrors ratatui-core's `CellWidth for str`
/// adjustment so this chokepoint and ratatui's own derivation land on the
/// same number for the same symbol (see `grapheme_width`'s doc below).
const HALFWIDTH_KATAKANA_VOICED_SOUND_MARK: char = '\u{FF9E}';
/// Halfwidth Katakana Semi-Voiced Sound Mark (handakuten) — the
/// `U+FF9F` counterpart to the mark above, same rationale.
const HALFWIDTH_KATAKANA_SEMI_VOICED_SOUND_MARK: char = '\u{FF9F}';

/// `ControlAwareWidth` — the single source of truth for a rune's display
/// width, shared (in the Go original) by the wrap/coordinate layer and the
/// cell renderer. Rule: `\n`/`\r` occupy no column; every other rune
/// reported zero-width is clamped to 1 (this is a DISPLAY-width decision
/// only — buffer bytes stay verbatim, §1.4.5). The 1-clamp exists so a LONE
/// zero-width rune (an isolated control char, a bare combining mark with no
/// base) still gets its own reachable caret column — see `grapheme_width`
/// below for why that reasoning does NOT extend to a rune that's part of a
/// larger grapheme cluster.
pub fn control_aware_width(r: char) -> usize {
    if r == '\n' || r == '\r' {
        return 0;
    }
    match unicode_width::UnicodeWidthChar::width(r) {
        Some(w) if w > 0 => w,
        _ => 1,
    }
}

/// The one tab stop every width walker expands against — `rune_width_with_tab`
/// and `grapheme_width_with_tab` both delegate to it rather than each
/// hardcoding `% 4`, so the two can never drift apart.
pub const TAB_STOP: usize = 4;

/// `runeWidthWithTab`: a tab expands to the next multiple-of-`TAB_STOP` stop.
pub fn rune_width_with_tab(r: char, current_width: usize) -> usize {
    if r == '\t' {
        return TAB_STOP - (current_width % TAB_STOP);
    }
    control_aware_width(r)
}

/// The display width of a WHOLE grapheme cluster (`unicode_segmentation::
/// graphemes`) — the second half of the shared width chokepoint alongside
/// `control_aware_width`/`rune_width_with_tab` above, used by BOTH
/// `wrap_line`'s greedy line-breaking (this module's sibling) and
/// `rune-tui`'s `render::push_grapheme_cells`/the sibling `query`
/// submodule's `WrapSnapshot::visual_col`/`byte_col_from_visual` — every
/// place that walks text one visual unit at a time, not one rune at a
/// time, must agree on this number or the caret lands on the wrong cell
/// (this module's own load-bearing property).
///
/// A single-rune cluster (plain ASCII, CJK, an isolated control char — the
/// overwhelming common case) delegates to `control_aware_width` unchanged.
///
/// A MULTI-rune cluster (a ZWJ-joined emoji sequence, a skin-tone-modified
/// emoji, a base char plus a variation selector or combining mark) is
/// measured with `unicode_width::UnicodeWidthStr::width` over the WHOLE
/// cluster, plus `+1` per halfwidth katakana dakuten/handakuten
/// (`U+FF9E`/`U+FF9F`) it contains, floored to a minimum of 1.
///
/// **The invariant this function exists to uphold: rune's width for a
/// symbol must equal what ratatui derives for that same symbol** — every
/// place that walks text one visual unit at a time (this module's
/// `wrap_line`, the `query` submodule's `WrapSnapshot::visual_col`/
/// `byte_col_from_visual`, and `rune-tui`'s `push_grapheme_cells`) has to
/// agree with the buffer ratatui actually diffs and writes to the
/// terminal, or a `Cell`'s reserved column count stops matching what the
/// terminal consumes for it — under-reserving drops a neighboring cell,
/// over-reserving shifts every cell after it on the row. rune-syntax stays
/// terminal-free and cannot depend on ratatui to assert this itself; the
/// equality is enforced by a guard test in `rune-tui`, which already
/// depends on both.
///
/// A per-rune MAX of `UnicodeWidthChar::width` (this function's prior
/// implementation) gets this wrong for a whole class of clusters —
/// variation selectors, regional-indicator flag pairs, keycaps, and
/// halfwidth katakana + dakuten all disagree with what
/// `UnicodeWidthStr::width` derives for the same string, because
/// `unicode-width` applies string-level ligature rules a per-`char` walk
/// cannot see. `UnicodeWidthStr::width` is NOT summing per-rune widths —
/// summing was tried and rejected (a real tmux capture against
/// `scripts/parity/fixtures/emoji.md` showed it reserves MORE columns than
/// the terminal actually consumes for a ZWJ-joined or skin-tone-modified
/// emoji, leaving a visible gap before whatever follows on the row).
/// `UnicodeWidthStr::width`'s ligature-aware width happens to still land on
/// the same answer as that empirical result for ZWJ/skin-tone clusters
/// (both count them as one double-wide glyph) while fixing the classes the
/// old MAX rule got wrong.
///
/// The result is clamped to a minimum of 1 (never zero), so a real buffer
/// byte always renders as at least one cell even for the pathological
/// all-zero-width cluster (`CELL-OFFSET`'s invariant, `rune-fuzz`).
pub fn grapheme_width(cluster: &str) -> usize {
    let mut chars = cluster.chars();
    let Some(first) = chars.next() else {
        return 0;
    };
    if chars.next().is_none() {
        return control_aware_width(first);
    }
    let halfwidth_marks = cluster
        .chars()
        .filter(|&c| {
            c == HALFWIDTH_KATAKANA_VOICED_SOUND_MARK
                || c == HALFWIDTH_KATAKANA_SEMI_VOICED_SOUND_MARK
        })
        .count();
    (unicode_width::UnicodeWidthStr::width(cluster) + halfwidth_marks).max(1)
}

/// `grapheme_width`'s tab-aware counterpart, mirroring `rune_width_with_tab`
/// — a tab is always its own single-rune cluster (grapheme segmentation
/// never joins a control char to a neighboring rune), so the two functions
/// agree exactly on every non-tab cluster and this only adds the 4-stop
/// tab-expansion case on top.
pub fn grapheme_width_with_tab(cluster: &str, current_width: usize) -> usize {
    if cluster == "\t" {
        return TAB_STOP - (current_width % TAB_STOP);
    }
    grapheme_width(cluster)
}

/// The next grapheme cluster in a row's concatenated span text, starting at
/// byte `pos`, clamped to never read past `bounds`' nearest entry greater
/// than `pos` (each entry is one `SyntaxSpan`'s own end offset within the
/// concatenation, ascending, last entry == `text.len()`).
///
/// This is the ONE place every width/column walk over a whole row's text
/// draws its cluster boundaries — `wrap_line`'s greedy breaker (this
/// module's sibling) and the coordinate queries in the `query` submodule
/// all call it rather than re-deriving boundaries via a bare
/// `graphemes(true)` over the concatenation. It exists because the code
/// that actually decides what lands in which terminal `Cell`
/// grapheme-segments each span's own text INDEPENDENTLY — a cluster's
/// `buf_offset`/style/`cell_map` lookup comes from exactly one span, never
/// two, so a cluster can never legitimately straddle a span boundary there.
/// A bare `graphemes(true)` walk over spans concatenated first has no such
/// boundary and will happily fuse characters across the seam: Unicode's own
/// cluster-break rules join a ZWJ (or any combining/extending rune) to
/// WHATEVER precedes it, span boundary or not (UAX #29 GB9 — unconditional,
/// unlike the pictographic-specific GB11 continuation rule the ZWJ sequence
/// itself relies on). Left unclamped, that fusion makes this module's width
/// sum diverge from the row's actual cells by exactly the fused cluster's
/// width whenever a span boundary happens to land right before such a rune
/// — silently, since both sides still individually look correct — and the
/// resulting column mismatch is what let a cursor's computed visual column
/// fail to land on any real cell (`CELL-ORDER`, `rune-fuzz`).
pub(super) fn next_grapheme<'a>(text: &'a str, bounds: &[usize], pos: usize) -> Option<&'a str> {
    let limit = bounds
        .iter()
        .copied()
        .find(|&b| b > pos)
        .unwrap_or(text.len());
    text.get(pos..limit)?.graphemes(true).next()
}
