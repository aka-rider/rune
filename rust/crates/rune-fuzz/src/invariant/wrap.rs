//! `WRAP-RT` (Go `WRAP-RT`) — `rune-md/tests/wrap_roundtrip.rs:88-97`'s own
//! domain, restated as a fuzz-time checker: the forward composition
//! `wrap_to_syntax(syntax_to_wrap(p))` is an identity for every `p` inside
//! the in-domain rectangle `line in 0..line_lens.len()`, `col in
//! 0..=line_lens[line]` (G7 — both functions silently clamp outside it,
//! and the REVERSE composition is deliberately not an identity at a wrap
//! seam).

use rune_core::coords::SyntaxPoint;
use rune_syntax::wrap::WrapSnapshot;

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

/// `WRAP-RT` (L0, sampled per G19) — see module docs for the domain.
/// Forward composition only: `wrap_to_syntax(syntax_to_wrap(sp)) == sp`
/// for every `sp` in the rectangle `line_lens` describes.
pub fn wrap_rt(wrap: &WrapSnapshot, line_lens: &[usize]) -> Option<Violation> {
    for (line, &len) in line_lens.iter().enumerate() {
        for col in 0..=len {
            let sp = SyntaxPoint { line, col };
            let wp = wrap.syntax_to_wrap(sp);
            let sp2 = wrap.wrap_to_syntax(wp);
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
