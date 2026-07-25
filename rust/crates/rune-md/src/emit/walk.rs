//! The recursive tree walk: `Block`/`Inline` -> `SyntaxSpan`s, via the
//! shared `push_span_split_by_line`/`hide_range` chokepoints in
//! `emit::mod`. Concealment is physical here, uniformly for block markers
//! AND inline delimiters: a `Rendered` element's marker/delimiter bytes are
//! dropped from the emitted text (recorded as a hidden range for
//! coordinate conversion) rather than kept-but-restyled — see the crate
//! root emit module docs for why this is a deliberate simplification of
//! Go's split block/inline concealment model.

use super::style::{StyleCtx, heading_style, list_marker_style, verbatim_style};
use super::syntax::SyntaxSpan;
use super::{Accounted, hide_range, push_span_split_by_line};
use crate::element::block::{Block, CodeFenceM, ListItemM};
use crate::element::inline::Inline;
use crate::element::{ByteRange, RevealState};
use crate::emit::style::StyleId;

fn emit_code_fence(
    content: &str,
    starts: &[usize],
    cf: &CodeFenceM,
    out: &mut [Vec<SyntaxSpan>],
    hidden: &mut Accounted,
    accounted: &mut Accounted,
) {
    if cf.sm.state() == RevealState::Revealed {
        push_span_split_by_line(
            content,
            starts,
            cf.range,
            StyleId::CodeFence,
            RevealState::Revealed,
            out,
            accounted,
        );
        return;
    }
    if let Some(open) = cf.fence_open {
        hide_range(hidden, accounted, content, starts, open);
    }
    if let Some(close) = cf.fence_close {
        hide_range(hidden, accounted, content, starts, close);
    }
    push_span_split_by_line(
        content,
        starts,
        cf.content,
        StyleId::CodeFence,
        RevealState::Rendered,
        out,
        accounted,
    );
}

fn emit_list_item(
    content: &str,
    starts: &[usize],
    item: &ListItemM,
    out: &mut [Vec<SyntaxSpan>],
    hidden: &mut Accounted,
    accounted: &mut Accounted,
) {
    if item.sm.state() == RevealState::Revealed {
        push_span_split_by_line(
            content,
            starts,
            item.marker,
            list_marker_style(item.task.is_some()),
            RevealState::Revealed,
            out,
            accounted,
        );
    } else {
        hide_range(hidden, accounted, content, starts, item.marker);
    }
    for c in &item.children {
        emit_block(content, starts, c, out, hidden, accounted);
    }
}

pub(crate) fn emit_block(
    content: &str,
    starts: &[usize],
    block: &Block,
    out: &mut [Vec<SyntaxSpan>],
    hidden: &mut Accounted,
    accounted: &mut Accounted,
) {
    match block {
        Block::Paragraph(p) => {
            emit_inlines(
                content,
                starts,
                &p.inlines,
                StyleCtx::default(),
                out,
                hidden,
                accounted,
            );
        }
        Block::Heading(h) => {
            if h.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    h.range,
                    heading_style(h.level),
                    RevealState::Revealed,
                    out,
                    accounted,
                );
            } else {
                hide_range(hidden, accounted, content, starts, h.marker);
                emit_inlines(
                    content,
                    starts,
                    &h.inlines,
                    StyleCtx::default(),
                    out,
                    hidden,
                    accounted,
                );
            }
        }
        Block::Blockquote(bq) => {
            for m in &bq.markers {
                if m.sm.state() == RevealState::Revealed {
                    push_span_split_by_line(
                        content,
                        starts,
                        m.marker,
                        StyleId::Blockquote,
                        RevealState::Revealed,
                        out,
                        accounted,
                    );
                } else {
                    hide_range(hidden, accounted, content, starts, m.marker);
                }
            }
            for c in &bq.children {
                emit_block(content, starts, c, out, hidden, accounted);
            }
        }
        Block::CodeFence(cf) => emit_code_fence(content, starts, cf, out, hidden, accounted),
        Block::List(list) => {
            for item in &list.items {
                emit_list_item(content, starts, item, out, hidden, accounted);
            }
        }
        Block::ThematicBreak(hr) => {
            if hr.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    hr.range,
                    StyleId::Hr,
                    RevealState::Revealed,
                    out,
                    accounted,
                );
            } else {
                hide_range(hidden, accounted, content, starts, hr.range);
            }
        }
        Block::Frontmatter(fm) => {
            push_span_split_by_line(
                content,
                starts,
                fm.range,
                StyleId::FrontmatterDim,
                RevealState::Revealed,
                out,
                accounted,
            );
        }
        Block::Verbatim(v) => {
            push_span_split_by_line(
                content,
                starts,
                v.range,
                verbatim_style(),
                RevealState::Revealed,
                out,
                accounted,
            );
        }
    }
}

/// The gap between a link's text and its `)` — `(open, close)` around the
/// visible label. A childless link (`[](url)`) has no text to bound the gap
/// on either side, so it contributes exactly ONE hidden range covering the
/// whole token (`open` = the full range, `close` = a zero-length range at
/// its end, which `hide_range`'s empty-range guard turns into a no-op) —
/// fixing the double-counted delimiter BLOCKER (an empty link previously
/// hid `[range.start, range.end)` twice, once as "open" and once as
/// "close"). Same shape as `parse.rs`'s `child_gap_delims` for a childless
/// emphasis node.
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

fn emit_inlines(
    content: &str,
    starts: &[usize],
    inlines: &[Inline],
    style_ctx: StyleCtx,
    out: &mut [Vec<SyntaxSpan>],
    hidden: &mut Accounted,
    accounted: &mut Accounted,
) {
    for inl in inlines {
        emit_inline(content, starts, inl, style_ctx, out, hidden, accounted);
    }
}

fn emit_inline(
    content: &str,
    starts: &[usize],
    inl: &Inline,
    style_ctx: StyleCtx,
    out: &mut [Vec<SyntaxSpan>],
    hidden: &mut Accounted,
    accounted: &mut Accounted,
) {
    match inl {
        Inline::Text(t) => {
            push_span_split_by_line(
                content,
                starts,
                t.range,
                style_ctx.resolve(),
                RevealState::Revealed,
                out,
                accounted,
            );
        }
        Inline::Emphasis(m) => {
            let child_ctx = style_ctx.with_kind(m.kind);
            if m.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    m.range,
                    child_ctx.resolve(),
                    RevealState::Revealed,
                    out,
                    accounted,
                );
            } else {
                hide_range(hidden, accounted, content, starts, m.open);
                emit_inlines(
                    content,
                    starts,
                    &m.children,
                    child_ctx,
                    out,
                    hidden,
                    accounted,
                );
                hide_range(hidden, accounted, content, starts, m.close);
            }
        }
        Inline::Code(m) => {
            if m.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    m.range,
                    StyleId::Code,
                    RevealState::Revealed,
                    out,
                    accounted,
                );
            } else {
                hide_range(hidden, accounted, content, starts, m.open);
                push_span_split_by_line(
                    content,
                    starts,
                    m.content,
                    StyleId::Code,
                    RevealState::Rendered,
                    out,
                    accounted,
                );
                hide_range(hidden, accounted, content, starts, m.close);
            }
        }
        Inline::Link(m) => {
            if m.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    m.range,
                    StyleId::Link,
                    RevealState::Revealed,
                    out,
                    accounted,
                );
            } else {
                let (open, close) = link_delims(m.range, &m.text);
                hide_range(hidden, accounted, content, starts, open);
                emit_inlines(
                    content,
                    starts,
                    &m.text,
                    StyleCtx::Override(StyleId::Link),
                    out,
                    hidden,
                    accounted,
                );
                hide_range(hidden, accounted, content, starts, close);
            }
        }
        Inline::WikiLink(m) => {
            if m.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    m.range,
                    StyleId::WikiLink,
                    RevealState::Revealed,
                    out,
                    accounted,
                );
            } else {
                let open = ByteRange::new(
                    m.range.start,
                    m.label.start.max(m.range.start).min(m.range.end),
                );
                let close =
                    ByteRange::new(m.label.end.max(m.range.start).min(m.range.end), m.range.end);
                hide_range(hidden, accounted, content, starts, open);
                push_span_split_by_line(
                    content,
                    starts,
                    m.label,
                    StyleId::WikiLink,
                    RevealState::Rendered,
                    out,
                    accounted,
                );
                hide_range(hidden, accounted, content, starts, close);
            }
        }
    }
}
