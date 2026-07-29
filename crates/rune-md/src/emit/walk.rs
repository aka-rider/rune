//! The recursive tree walk: `Block`/`Inline` -> `SyntaxSpan`s, via the
//! shared `push_span_split_by_line`/`hide_range`/`claim_visible` chokepoints
//! in `emit::mod`. Concealment is physical here, uniformly for block markers
//! AND inline delimiters: a `Rendered` element's marker/delimiter bytes are
//! dropped from the emitted text (recorded as a hidden range for
//! coordinate conversion) rather than kept-but-restyled — see the crate
//! root emit module docs for why this is a deliberate simplification of
//! Go's split block/inline concealment model.
//!
//! Every `emit_block`/`emit_inline` call threads one `&mut EmitOut` (WP2.S3)
//! instead of three loose out-params (`spans`, `hidden`, `accounted`) plus a
//! fourth WP2 added (`tables`) — the repo bans
//! `#[allow(clippy::too_many_arguments)]`.

use super::style::{
    StyleCtx, blockquote_scope, code_fence_scope, code_scope, frontmatter_scope, heading_style,
    hr_scope, link_scope, list_marker_style, verbatim_style,
};
use super::table::emit_table;
use super::{
    Accounted, EmitOut, assert_invariant, claim_visible, hide_range, push_span_split_by_line,
};
use crate::element::block::{Block, CodeFenceM, ListItemM};
use crate::element::inline::Inline;
use crate::parse::line_at;
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
fn emit_code_fence(content: &str, starts: &[usize], cf: &CodeFenceM, out: &mut EmitOut) {
    if cf.sm.state() == RevealState::Revealed {
        if let Some(open) = cf.fence_open {
            push_span_split_by_line(
                content,
                starts,
                open,
                code_fence_scope(),
                RevealState::Revealed,
                out.spans,
                out.accounted,
            );
        }
        for &line in &cf.content_lines {
            push_span_split_by_line(
                content,
                starts,
                line,
                code_fence_scope(),
                RevealState::Revealed,
                out.spans,
                out.accounted,
            );
        }
        if let Some(close) = cf.fence_close {
            push_span_split_by_line(
                content,
                starts,
                close,
                code_fence_scope(),
                RevealState::Revealed,
                out.spans,
                out.accounted,
            );
        }
        return;
    }
    if let Some(open) = cf.fence_open {
        hide_range(out.hidden, out.accounted, content, starts, open);
    }
    if let Some(close) = cf.fence_close {
        hide_range(out.hidden, out.accounted, content, starts, close);
    }
    for &line in &cf.content_lines {
        push_span_split_by_line(
            content,
            starts,
            line,
            code_fence_scope(),
            RevealState::Rendered,
            out.spans,
            out.accounted,
        );
    }
}

fn emit_list_item(content: &str, starts: &[usize], item: &ListItemM, out: &mut EmitOut) {
    if item.sm.state() == RevealState::Revealed {
        push_span_split_by_line(
            content,
            starts,
            item.marker,
            list_marker_style(item.task.is_some()),
            RevealState::Revealed,
            out.spans,
            out.accounted,
        );
    } else if let Some(task) = item.task {
        // Go parity (`walkTaskList`): a task item's checkbox substitutes to a glyph even
        // while concealed — plain bullet/ordered markers (the `else` arm
        // below) stay fully hidden, the Rust-only "list markers are always
        // concealed" divergence recorded in `scripts/parity/README.md`.
        // The "- "/"1. " prefix before the checkbox is hidden exactly like
        // a plain marker; only the checkbox itself substitutes.
        let before = ByteRange::new(item.marker.start, task.start);
        hide_range(out.hidden, out.accounted, content, starts, before);
        push_task_checkbox(content, starts, task, out.spans, out.hidden, out.accounted);
        // Whatever sits between the checkbox and the item's own content
        // (normally exactly one space) is deliberately left UNCLAIMED here:
        // `fill_gaps` (`emit/mod.rs`) supplies it verbatim as an ordinary
        // `Identical` span, so 0/1/N trailing spaces round-trip exactly —
        // see `push_task_checkbox`'s docs for why this keeps the
        // substitution byte-length-neutral against `SyntaxSnapshot`'s
        // hidden-ranges-only coordinate model instead of hand-rolling a
        // second hidden-range delta for the difference.
    } else {
        hide_range(out.hidden, out.accounted, content, starts, item.marker);
    }
    for c in &item.children {
        emit_block(content, starts, c, out);
    }
}

/// Substitutes a task item's `"[ ]"`/`"[x]"`/`"[X]"` — always exactly 3
/// bytes (`ListItemM::task`'s docs) — with its checkbox glyph: `☐`
/// (U+2610) unchecked, `☑` (U+2611) checked. Go parity (`walkTaskList`).
///
/// Deliberately NOT routed through `hide_range` — this substitutes visible
/// content, it doesn't hide it — and deliberately NOT built via
/// `push_span_split_by_line` (which only ever copies `content[range]`
/// itself into `Substituted::text`, never a genuinely different string).
/// Routes the claim itself through `claim_visible` — the same
/// unclaimed-subranges-plus-assert chokepoint every other own-text
/// producer in this crate uses — instead of writing `out`/`accounted`
/// directly, so an overlapping claim here is clipped and reported instead
/// of silently invented.
///
/// Byte-length-preserving BY CONSTRUCTION, which is why this needs no
/// extra hidden-range bookkeeping: `☐`/`☑` are each exactly 3 bytes in
/// UTF-8 (codepoints `U+2610`/`U+2611`, the 3-byte range), the SAME length
/// as the 3-byte ASCII `task` range they replace. The buffer<->syntax
/// coordinate model only ever accounts for FULLY hidden byte ranges
/// (`hide_range`) — it has no notion of a visible span whose substituted
/// text is a different byte length than the buffer range it replaces — so
/// a length-changing substitution here would desync every position later
/// on the same line. Keeping this specific substitution exactly 3-for-3
/// bytes sidesteps that entirely: no hidden delta is needed, and every
/// later byte-indexed width/column walk built from `SyntaxSpan::text`
/// (i.e. this span's own 3-byte `text`) stays consistent with it for free
/// — a precondition this function now checks rather than assumes:
/// `assert_invariant` surfaces a producer that hands it a `task` range of
/// any other length, and (in every build, not just a strict one) it falls
/// through to hiding the range like a plain marker instead of emitting a
/// glyph whose byte length would lie about the range it replaces.
fn push_task_checkbox(
    content: &str,
    starts: &[usize],
    task: ByteRange,
    out: &mut [Vec<SyntaxSpan>],
    hidden: &mut Accounted,
    accounted: &mut Accounted,
) {
    let Some(bytes) = content.get(task.start..task.end) else {
        return;
    };
    assert_invariant(task.len() == 3, || {
        format!(
            "task checkbox range [{},{}) is {} bytes, not the 3-byte \"[ ]\"/\"[x]\" ListItemM::task's own docs promise — producer bug",
            task.start,
            task.end,
            task.len()
        )
    });
    if task.len() != 3 {
        // Can't substitute a glyph whose byte length would disagree with
        // the range it replaces (this function's own docs) — fall through
        // to hiding it verbatim, exactly like a plain, non-task marker.
        hide_range(hidden, accounted, content, starts, task);
        return;
    }
    let checked = bytes.as_bytes().get(1).is_some_and(|&b| b != b' ');
    let glyph = if checked { "\u{2611}" } else { "\u{2610}" };
    let line = line_at(starts, task.start);

    let pieces = claim_visible(accounted, line, task.start, task.end);
    if pieces != [(task.start, task.end)] {
        // The whole range was not cleanly unclaimed (an overlap
        // `claim_visible`'s own assert already flagged) — there is no
        // half-glyph substitution, so skip rather than desync the
        // byte-length-neutral invariant this function exists for.
        return;
    }

    let span = SyntaxSpan::Substituted {
        // Pre-WP4 this was `StyleId::TaskMarker`; WP4 folded that variant
        // into `list_marker_style`'s task arm ("markup.list.checked").
        scope: list_marker_style(true),
        text: glyph.to_string(),
        range: task.start..task.end,
        cell_map: vec![task.start as i64],
    };
    if let Some(bucket) = out.get_mut(line) {
        bucket.push(span);
    }
}

pub(crate) fn emit_block(content: &str, starts: &[usize], block: &Block, out: &mut EmitOut) {
    match block {
        Block::Paragraph(p) => {
            emit_inlines(content, starts, &p.inlines, StyleCtx::default(), out);
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
                        out.spans,
                        out.accounted,
                    );
                }
            } else {
                hide_range(out.hidden, out.accounted, content, starts, h.marker);
                emit_inlines(content, starts, &h.inlines, StyleCtx::default(), out);
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
                        out.spans,
                        out.accounted,
                    );
                } else {
                    hide_range(out.hidden, out.accounted, content, starts, m.marker);
                }
            }
            for c in &bq.children {
                emit_block(content, starts, c, out);
            }
        }
        Block::CodeFence(cf) => emit_code_fence(content, starts, cf, out),
        Block::List(list) => {
            for item in &list.items {
                emit_list_item(content, starts, item, out);
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
                    out.spans,
                    out.accounted,
                );
            } else {
                hide_range(out.hidden, out.accounted, content, starts, hr.range);
            }
        }
        Block::Frontmatter(fm) => {
            push_span_split_by_line(
                content,
                starts,
                fm.range,
                frontmatter_scope(),
                RevealState::Revealed,
                out.spans,
                out.accounted,
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
                    out.spans,
                    out.accounted,
                );
            }
        }
        Block::Table(t) => emit_table(content, starts, t, out),
    }
}

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

fn emit_inlines(
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
                        out.spans,
                        out.accounted,
                    );
                }
            } else {
                hide_range(out.hidden, out.accounted, content, starts, m.open);
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
                        out.spans,
                        out.accounted,
                    );
                }
                hide_range(out.hidden, out.accounted, content, starts, m.close);
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
    }
}
