//! The inline-node handler family, split out from the block-node walk to
//! keep the owning module under the 500-line budget: every
//! `Inline` variant's own concealment arm, plus the link-delimiter gap
//! helper only that arm needs. Shares the same `push_span_split_by_line`/
//! `hide_range` chokepoints as the block-node walk.

use super::style::{StyleCtx, code_scope, image_scope, link_scope};
use super::{EmitOut, hide_range, push_span_split_by_line};
use crate::element::inline::Inline;
use rune_syntax::element::{ByteRange, RevealState};

/// The gap between a link's text and its `)` — `(open, close)` around the
/// visible label. A childless link (`[](url)`) has no text to bound the gap
/// on either side, so it contributes exactly ONE hidden range covering the
/// whole token (`open` = the full range, `close` = a zero-length range at
/// its end, which `hide_range`'s empty-range guard turns into a no-op) —
/// fixing the double-counted delimiter BLOCKER (an empty link previously
/// hid `[range.start, range.end)` twice, once as "open" and once as
/// "close"). Same shape as the parser's own `child_gap_delims` for a
/// childless emphasis node.
fn link_delims(range: ByteRange, children: &[Inline]) -> (ByteRange, ByteRange) {
    match (children.first(), children.last()) {
        (Some(first), Some(last)) => {
            let open_end = first.range().start.max(range.start).min(range.end);
            let close_start = last.range().end.max(range.start).min(range.end);
            (
                ByteRange::new(range.start, open_end),
                ByteRange::new(close_start, range.end),
            )
        }
        _ => (range, ByteRange::new(range.end, range.end)),
    }
}

pub(crate) fn emit_inlines(
    content: &str,
    starts: &[usize],
    inlines: &[Inline],
    style_ctx: StyleCtx,
    out: &mut EmitOut,
) {
    for inl in inlines {
        emit_inline(content, starts, inl, style_ctx, out);
    }
}

fn emit_inline(
    content: &str,
    starts: &[usize],
    inl: &Inline,
    style_ctx: StyleCtx,
    out: &mut EmitOut,
) {
    match inl {
        Inline::Text(t) => {
            // MAJOR fix (verification round 9): `t.content_lines` — never
            // `t.range` directly — the same reason `Block::Verbatim`'s
            // (and `CodeFenceM`'s) emission iterates its own content
            // lines instead of pushing one contiguous range (see this
            // file's `CodeFence`/`Verbatim` docs): an unmodeled inline
            // node's `range` alone can span a container's own repeating
            // prefix on a continuation line, which a single contiguous
            // push can't exclude.
            for &line in &t.content_lines {
                push_span_split_by_line(
                    content,
                    starts,
                    line,
                    style_ctx.resolve(),
                    RevealState::Revealed,
                    out.spans,
                    out.accounted,
                );
            }
        }
        Inline::Emphasis(m) => {
            let child_ctx = style_ctx.with_kind(m.kind);
            if m.sm.state() == RevealState::Revealed {
                // MAJOR fix (verification round 9's exhaustive audit):
                // `m.content_lines` — never `m.range` directly — the
                // same reason `Block::Verbatim`'s emission iterates its
                // own content lines (see this file's `Verbatim` docs):
                // emphasis/strong/strikethrough content can soft-wrap
                // across lines, and `range` alone can't exclude a
                // container's own repeating prefix on the continuation
                // line.
                for &line in &m.content_lines {
                    push_span_split_by_line(
                        content,
                        starts,
                        line,
                        child_ctx.resolve(),
                        RevealState::Revealed,
                        out.spans,
                        out.accounted,
                    );
                }
            } else {
                hide_range(out.hidden, out.accounted, content, starts, m.open);
                emit_inlines(content, starts, &m.children, child_ctx, out);
                hide_range(out.hidden, out.accounted, content, starts, m.close);
            }
        }
        Inline::Code(m) => {
            if m.state() == RevealState::Revealed {
                for &line in m.content_lines() {
                    push_span_split_by_line(
                        content,
                        starts,
                        line,
                        code_scope(),
                        RevealState::Revealed,
                        out.spans,
                        out.accounted,
                    );
                }
            } else {
                hide_range(out.hidden, out.accounted, content, starts, m.open());
                for &line in m.inner_lines() {
                    push_span_split_by_line(
                        content,
                        starts,
                        line,
                        code_scope(),
                        RevealState::Rendered,
                        out.spans,
                        out.accounted,
                    );
                }
                hide_range(out.hidden, out.accounted, content, starts, m.close());
            }
        }
        Inline::Link(m) => {
            if m.sm.state() == RevealState::Revealed {
                // MAJOR fix (verification round 9): `m.content_lines`,
                // matching `Emphasis`'s own revealed-path fix above.
                for &line in &m.content_lines {
                    push_span_split_by_line(
                        content,
                        starts,
                        line,
                        link_scope(),
                        RevealState::Revealed,
                        out.spans,
                        out.accounted,
                    );
                }
            } else {
                let (open, close) = link_delims(m.range, &m.text);
                hide_range(out.hidden, out.accounted, content, starts, open);
                emit_inlines(
                    content,
                    starts,
                    &m.text,
                    StyleCtx::Override(link_scope()),
                    out,
                );
                hide_range(out.hidden, out.accounted, content, starts, close);
            }
        }
        Inline::WikiLink(m) => {
            if m.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    m.range,
                    link_scope(),
                    RevealState::Revealed,
                    out.spans,
                    out.accounted,
                );
            } else {
                let open = ByteRange::new(
                    m.range.start,
                    m.label.start.max(m.range.start).min(m.range.end),
                );
                let close =
                    ByteRange::new(m.label.end.max(m.range.start).min(m.range.end), m.range.end);
                hide_range(out.hidden, out.accounted, content, starts, open);
                push_span_split_by_line(
                    content,
                    starts,
                    m.label,
                    link_scope(),
                    RevealState::Rendered,
                    out.spans,
                    out.accounted,
                );
                hide_range(out.hidden, out.accounted, content, starts, close);
            }
        }
        Inline::Image(m) => {
            if m.sm.state() == RevealState::Revealed {
                for &line in &m.content_lines {
                    push_span_split_by_line(
                        content,
                        starts,
                        line,
                        image_scope(),
                        RevealState::Revealed,
                        out.spans,
                        out.accounted,
                    );
                }
            } else {
                // Empty alt (or an empty wikilink-style target, which never
                // happens by construction but is handled the same way
                // regardless) falls back to the target itself as the
                // visible label — the "empty alt, URL becomes the visible
                // label" rule, same shape `WikiLinkM`'s own label/open/close
                // split above already uses.
                let label = if m.alt.is_empty() { m.target } else { m.alt };
                let open = ByteRange::new(
                    m.range.start,
                    label.start.max(m.range.start).min(m.range.end),
                );
                let close =
                    ByteRange::new(label.end.max(m.range.start).min(m.range.end), m.range.end);
                hide_range(out.hidden, out.accounted, content, starts, open);
                push_span_split_by_line(
                    content,
                    starts,
                    label,
                    image_scope(),
                    RevealState::Rendered,
                    out.spans,
                    out.accounted,
                );
                hide_range(out.hidden, out.accounted, content, starts, close);
            }
        }
    }
}
