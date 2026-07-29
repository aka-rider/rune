//! `WRAP-RT` — `rune-md`'s own `wrap_roundtrip` proptest domain, restated
//! as a fuzz-time checker: the forward composition
//! `wrap_to_syntax(syntax_to_wrap(p))` is an identity for every `p` inside
//! the in-domain rectangle `line in 0..line_lens.len()`, `col in
//! 0..=line_lens[line]` (G7 — both functions silently clamp outside it,
//! and the REVERSE composition is deliberately not an identity at a wrap
//! seam). WP11.S3 narrowed what "in-domain" means: `wrap_to_syntax` now
//! snaps a byte column that lands mid-grapheme-cluster (e.g. inside the
//! 3-byte checkbox glyph `push_task_checkbox` substitutes) down to that
//! cluster's start rather than passing it through — a real cursor position
//! can never land there, so `col in 0..=line_lens[line]` alone is too wide
//! a domain to promise identity over; only cluster-boundary-aligned
//! columns are. `wrap_rt` below still always probes `line_lens[line]`
//! itself even when it isn't boundary-aligned, so a deliberately corrupted
//! (too-large) bound still trips a violation the way it always has —
//! see `wrap_rt_detects_an_out_of_domain_bound`.

use rune_core::coords::SyntaxPoint;
use rune_syntax::wrap::WrapSnapshot;
use unicode_segmentation::UnicodeSegmentation;

use super::Violation;

/// The exact in-domain upper bound per line: the summed byte length of
/// that model line's syntax-space text, computed by walking its wrap
/// segments. `ViewSnapshots` doesn't carry the emitter's raw `SyntaxLine`s
/// separately (G16), so this recovers the same number `rune-md/tests/
/// wrap_roundtrip.rs`'s `syntax_line_byte_len` derives straight from them,
/// but from the `WrapSnapshot` this crate already has. Bounded: `row`
/// only ever advances while `row < wrap.total_rows()`, so a malformed
/// `WrapSnapshot` can never spin this forever.
///
/// Caller-supplied to `wrap_rt` (not baked into it) so a test can hand it a
/// deliberately wrong bound to fabricate a violation without needing to
/// construct a broken `WrapSnapshot` — its fields are private to
/// `rune-md`.
pub fn wrap_line_lens(wrap: &WrapSnapshot, line_count: usize) -> Vec<usize> {
    let total_rows = wrap.total_rows();
    (0..line_count)
        .map(|line| {
            let mut len = 0usize;
            let mut row = wrap.model_line_to_first_row(line);
            while row < total_rows && wrap.row_to_model_line(row) == line {
                len += wrap.segment_len_at(row);
                row += 1;
            }
            len
        })
        .collect()
}

/// `line`'s syntax-space text, reconstructed by walking its wrap segments
/// and concatenating each span's own visible text — the same text
/// `wrap_line_lens` sums the byte length of, recovered here in full so
/// `wrap_rt` can find its actual grapheme-cluster boundaries.
fn line_text(content: &str, wrap: &WrapSnapshot, line: usize) -> String {
    let total_rows = wrap.total_rows();
    let mut row = wrap.model_line_to_first_row(line);
    let mut text = String::new();
    while row < total_rows && wrap.row_to_model_line(row) == line {
        if let Some(seg) = wrap.segments().get(row) {
            for sp in &seg.spans {
                text.push_str(sp.text(content));
            }
        }
        row += 1;
    }
    text
}

/// `WRAP-RT` (L0, sampled per G19) — see module docs for the domain.
/// Forward composition only: `wrap_to_syntax(syntax_to_wrap(sp)) == sp`
/// for every `sp` whose `col` is a grapheme-cluster boundary of `line`'s
/// own syntax-space text, up to (and including) `line_lens[line]` — plus
/// `line_lens[line]` itself even when a corrupted bound isn't
/// boundary-aligned, so a deliberately-too-large bound still trips a
/// violation (module docs).
pub fn wrap_rt(content: &str, wrap: &WrapSnapshot, line_lens: &[usize]) -> Option<Violation> {
    for (line, &len) in line_lens.iter().enumerate() {
        let text = line_text(content, wrap, line);
        let mut cols: Vec<usize> = text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .filter(|&c| c <= len)
            .collect();
        if !cols.contains(&len) {
            cols.push(len);
        }
        for col in cols {
            let sp = SyntaxPoint { line, col };
            let wp = wrap.syntax_to_wrap(sp);
            let sp2 = wrap.wrap_to_syntax(content, wp);
            if sp2 != sp {
                return Some(Violation {
                    id: "WRAP-RT",
                    message: format!(
                        "wrap_to_syntax(syntax_to_wrap({sp:?})) = {sp2:?}, want {sp:?} (via {wp:?})"
                    ),
                });
            }
        }
    }
    None
}
