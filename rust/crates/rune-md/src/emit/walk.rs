//! The recursive tree walk: `Block`/`Inline` -> `SyntaxSpan`s, via the
//! shared `push_span_split_by_line`/`hide_range` chokepoints in
//! `emit::mod`. Concealment is physical here, uniformly for block markers
//! AND inline delimiters: a `Rendered` element's marker/delimiter bytes are
//! dropped from the emitted text (recorded as a hidden range for
//! coordinate conversion) rather than kept-but-restyled — see the crate
//! root emit module docs for why this is a deliberate simplification of
//! Go's split block/inline concealment model.

use super::style::{
    StyleCtx, blockquote_scope, code_fence_scope, code_scope, frontmatter_scope, heading_style,
    hr_scope, link_scope, list_marker_style, verbatim_style,
};
use super::{Accounted, hide_range, push_span_split_by_line};
use crate::element::block::{Block, CodeFenceM, ListItemM};
use crate::element::inline::Inline;
use rune_syntax::SyntaxSpan;
use rune_syntax::element::{ByteRange, RevealState};

/// Every piece here (`fence_open`, each of `content_lines`, `fence_close`)
/// is already exactly one physical line's range, computed container-aware
/// at parse time (`parse::block`'s `CodeBlock` arm) — never re-derived from
/// `cf.range` as one contiguous multi-line span. `cf.range` spans every
/// line the fence occupies INCLUDING any repeating container prefix
/// (blockquote's `"> "`) on continuation lines; pushing it whole through
/// the generic per-physical-line splitter (as the Revealed path used to)
/// re-hides/re-shows those container-prefix bytes a second time, on top of
/// whatever the container itself already claimed for that line.
fn emit_code_fence(
    content: &str,
    starts: &[usize],
    cf: &CodeFenceM,
    out: &mut [Vec<SyntaxSpan>],
    hidden: &mut Accounted,
    accounted: &mut Accounted,
) {
    if cf.sm.state() == RevealState::Revealed {
        if let Some(open) = cf.fence_open {
            push_span_split_by_line(
                content,
                starts,
                open,
                code_fence_scope(),
                RevealState::Revealed,
                out,
                accounted,
            );
        }
        for &line in &cf.content_lines {
            push_span_split_by_line(
                content,
                starts,
                line,
                code_fence_scope(),
                RevealState::Revealed,
                out,
                accounted,
            );
        }
        if let Some(close) = cf.fence_close {
            push_span_split_by_line(
                content,
                starts,
                close,
                code_fence_scope(),
                RevealState::Revealed,
                out,
                accounted,
            );
        }
        return;
    }
    if let Some(open) = cf.fence_open {
        hide_range(hidden, accounted, content, starts, open);
    }
    if let Some(close) = cf.fence_close {
        hide_range(hidden, accounted, content, starts, close);
    }
    for &line in &cf.content_lines {
        push_span_split_by_line(
            content,
            starts,
            line,
            code_fence_scope(),
            RevealState::Rendered,
            out,
            accounted,
        );
    }
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
                // MAJOR fix (verification round 4): `h.content_lines` —
                // never `h.range` directly — the same reason
                // `emit_code_fence` iterates `cf.content_lines` instead of
                // pushing `cf.range` whole (see this file's CodeFence
                // docs): `range` alone can span a container's own
                // repeating prefix on a setext heading's underline line,
                // which a single contiguous push can't exclude.
                for &line in &h.content_lines {
                    push_span_split_by_line(
                        content,
                        starts,
                        line,
                        heading_style(h.level),
                        RevealState::Revealed,
                        out,
                        accounted,
                    );
                }
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
                        blockquote_scope(),
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
                    hr_scope(),
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
                frontmatter_scope(),
                RevealState::Revealed,
                out,
                accounted,
            );
        }
        Block::Verbatim(v) => {
            // MAJOR fix (verification round 9): `v.content_lines` — never
            // `v.range` directly — the same reason `emit_code_fence`
            // iterates `cf.content_lines` instead of pushing `cf.range`
            // whole (see this file's `CodeFence` docs): `range` alone can
            // span a container's own repeating prefix on a table/HTML-
            // block/unknown construct's continuation line, which a
            // single contiguous push can't exclude.
            for &line in &v.content_lines {
                push_span_split_by_line(
                    content,
                    starts,
                    line,
                    verbatim_style(),
                    RevealState::Revealed,
                    out,
                    accounted,
                );
            }
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
                    out,
                    accounted,
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
                        out,
                        accounted,
                    );
                }
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
                // MAJOR fix (verification round 9): `m.content_lines`,
                // matching `Emphasis`'s own revealed-path fix above.
                for &line in &m.content_lines {
                    push_span_split_by_line(
                        content,
                        starts,
                        line,
                        code_scope(),
                        RevealState::Revealed,
                        out,
                        accounted,
                    );
                }
            } else {
                hide_range(hidden, accounted, content, starts, m.open);
                // MAJOR fix (verification round 9): `m.inner_lines` —
                // never `m.content` directly — a code span's INNER text
                // can soft-wrap across lines exactly like its outer
                // `range` can (verified empirically: "> `a\n> b`" used
                // to re-claim the continuation line's own "> " marker as
                // part of the code span's rendered content).
                for &line in &m.inner_lines {
                    push_span_split_by_line(
                        content,
                        starts,
                        line,
                        code_scope(),
                        RevealState::Rendered,
                        out,
                        accounted,
                    );
                }
                hide_range(hidden, accounted, content, starts, m.close);
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
                        out,
                        accounted,
                    );
                }
            } else {
                let (open, close) = link_delims(m.range, &m.text);
                hide_range(hidden, accounted, content, starts, open);
                emit_inlines(
                    content,
                    starts,
                    &m.text,
                    StyleCtx::Override(link_scope()),
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
                    link_scope(),
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
                    link_scope(),
                    RevealState::Rendered,
                    out,
                    accounted,
                );
                hide_range(hidden, accounted, content, starts, close);
            }
        }
    }
}
