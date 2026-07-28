//! Cell rendering (WP2.S6): a table cell's own `inlines` walked with the
//! SAME concealment `emit::walk`'s `emit_inline` applies to ordinary
//! paragraph text — emphasis/code/link delimiters dropped, link text kept,
//! nested emphasis resolved via the shared `emit::style::StyleCtx` — so a
//! cell's rendering never drifts from how the exact same markup renders
//! outside a table. Reuses `emit::style`'s scope resolver (`pub(crate)` to
//! this crate specifically so a sibling module can reach it) rather than
//! re-registering a second one.
//!
//! **comrak's cell sourcepos includes the padding spaces**, not the
//! trimmed word (measured against comrak 0.54.0 and pinned by the table
//! model's own integration tests) — irrelevant here, since this module
//! never reads `TableCellM::range` directly; it walks `cell.inlines`
//! (already trimmed the same way a paragraph's inlines are) instead.

use crate::element::inline::Inline;
use crate::element::table::TableCellM;
use crate::emit::style::{self, StyleCtx};
use rune_syntax::ScopeId;
use rune_syntax::element::{ByteRange, RevealState};

use super::CellSrc;

/// One cell's rendered content: `text` is what's shown, `src` carries one
/// [`CellSrc`] per char of `text` (`text.chars().count() == src.len()`,
/// the same per-CHAR indexing `SyntaxSpan::Substituted::cell_map` requires
/// crate-wide).
pub struct RenderedCell {
    pub text: String,
    pub src: Vec<CellSrc>,
}

impl RenderedCell {
    fn push_str(&mut self, content: &str, range: ByteRange, scope: ScopeId) {
        let Some(text) = content.get(range.start..range.end) else {
            return;
        };
        for (i, ch) in text.char_indices() {
            // A tab is display-substituted with a single space. Column
            // widths here are measured per grapheme, where a tab counts as
            // one cell, but the terminal renderer expands a surviving tab to
            // the next 4-column stop — so a tab inside a cell would draw a
            // row wider than the geometry every other row was laid out to,
            // and the box would not line up. Substituting keeps the
            // char-for-char mapping intact (one char in, one char out, so
            // `src` stays index-aligned) and leaves the buffer bytes
            // untouched (§1.4.5) — this is a display decision only.
            self.text.push(if ch == '\t' { ' ' } else { ch });
            self.src.push(CellSrc {
                buf: (range.start + i) as i64,
                scope,
            });
        }
    }
}

/// A [`StyleCtx`] resolves nested emphasis to a real scope but falls back
/// to the crate's generic `"text"` scope for plain, unstyled content
/// (`emit::style::StyleCtx::resolve`'s own docs) — inside a table cell,
/// that fallback should be the ROW's own role scope (header/body) instead,
/// so a cell with no emphasis at all still gets the table's styling rather
/// than plain-paragraph styling. Emphasis/code/link scopes are left exactly
/// as `StyleCtx::resolve` (or the code/link scope helpers) produced them.
fn cell_scope(style_ctx: StyleCtx, base: ScopeId) -> ScopeId {
    let resolved = style_ctx.resolve();
    if resolved == style::text_scope() {
        base
    } else {
        resolved
    }
}

/// Renders one table cell's `inlines` into `(text, per-char CellSrc)` —
/// `base` is the row's role scope (`emit::style::table_header_scope()` for
/// a header row, `table_scope()` for a body row), substituted for any
/// plain-text run that would otherwise fall back to the generic `"text"`
/// scope (see [`cell_scope`]).
pub fn render_cell(content: &str, cell: &TableCellM, base: ScopeId) -> RenderedCell {
    let mut out = RenderedCell {
        text: String::new(),
        src: Vec::new(),
    };
    render_inlines(content, &cell.inlines, StyleCtx::default(), base, &mut out);
    out
}

fn render_inlines(
    content: &str,
    inlines: &[Inline],
    style_ctx: StyleCtx,
    base: ScopeId,
    out: &mut RenderedCell,
) {
    for inl in inlines {
        render_inline(content, inl, style_ctx, base, out);
    }
}

/// Mirrors `emit::walk::emit_inline` arm-for-arm, producing `(text,
/// CellSrc)` pairs instead of pushing `SyntaxSpan`s: `Revealed` shows the
/// node's own raw content-lines range styled with the current context (same
/// as the paragraph path); `Rendered` drops delimiter bytes and recurses/
/// substitutes exactly like the paragraph emitter does. Reachable `Revealed`
/// states are unreachable in practice while the enclosing table itself is
/// `Rendered` (WP2's whole-block reveal policy, plan architectural decision
/// 5: the table's own cursor-in-range check already forces every
/// descendant inline's own check to agree), but mirroring the full arm set
/// keeps this function correct independent of that invariant rather than
/// silently relying on it.
fn render_inline(
    content: &str,
    inl: &Inline,
    style_ctx: StyleCtx,
    base: ScopeId,
    out: &mut RenderedCell,
) {
    match inl {
        Inline::Text(t) => {
            let scope = cell_scope(style_ctx, base);
            for &line in &t.content_lines {
                out.push_str(content, line, scope);
            }
        }
        Inline::Emphasis(m) => {
            let child_ctx = style_ctx.with_kind(m.kind);
            if m.sm.state() == RevealState::Revealed {
                let scope = cell_scope(child_ctx, base);
                for &line in &m.content_lines {
                    out.push_str(content, line, scope);
                }
            } else {
                render_inlines(content, &m.children, child_ctx, base, out);
            }
        }
        Inline::Code(m) => {
            let scope = style::code_scope();
            if m.sm.state() == RevealState::Revealed {
                for &line in &m.content_lines {
                    out.push_str(content, line, scope);
                }
            } else {
                for &line in &m.inner_lines {
                    out.push_str(content, line, scope);
                }
            }
        }
        Inline::Link(m) => {
            let scope = style::link_scope();
            if m.sm.state() == RevealState::Revealed {
                for &line in &m.content_lines {
                    out.push_str(content, line, scope);
                }
            } else {
                render_inlines(content, &m.text, StyleCtx::Override(scope), base, out);
            }
        }
        Inline::WikiLink(m) => {
            let scope = style::link_scope();
            if m.sm.state() == RevealState::Revealed {
                out.push_str(content, m.range, scope);
            } else {
                out.push_str(content, m.label, scope);
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::element::block::Block;
    use crate::parse::parse;
    use rune_syntax::scope::markdown_table;

    fn header_scope() -> ScopeId {
        markdown_table()
            .resolve("markup.table.header")
            .expect("scope registered")
    }

    fn only_table(content: &str) -> crate::element::table::TableM {
        let blocks = parse(content);
        let Block::Table(t) = blocks.into_iter().next().expect("one block") else {
            panic!("expected Block::Table");
        };
        t
    }

    #[test]
    fn plain_cell_renders_verbatim_with_role_scope() {
        let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n";
        let t = only_table(content);
        let base = header_scope();
        let rendered = render_cell(content, &t.rows[0].cells[0], base);
        assert_eq!(rendered.text, "Name");
        assert_eq!(rendered.src.len(), 4);
        for src in &rendered.src {
            assert_eq!(src.scope, base);
        }
    }

    #[test]
    fn cell_src_len_matches_char_count_for_multibyte_text() {
        let content = "| 世界 | b |\n| --- | --- |\n| x | y |\n";
        let t = only_table(content);
        let base = header_scope();
        let rendered = render_cell(content, &t.rows[0].cells[0], base);
        assert_eq!(rendered.text, "世界");
        assert_eq!(rendered.src.len(), rendered.text.chars().count());
    }
}
