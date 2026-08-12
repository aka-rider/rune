//! The recursive tree walk: `Block`/`Inline` -> `SyntaxSpan`s, via the
//! shared `push_span_split_by_line`/`hide_range`/`EmitOut::claim_free`
//! chokepoints in `emit::mod`. Concealment is physical here, uniformly for
//! block markers AND inline delimiters: a `Rendered` element's
//! marker/delimiter bytes are dropped from the emitted text (recorded as a
//! hidden range for coordinate conversion) rather than kept-but-restyled —
//! see the crate root emit module docs for why this unifies block and
//! inline concealment under one policy.
//!
//! Every `emit_block`/`emit_inline` call threads one `&mut EmitOut` (WP2.S3)
//! instead of three loose out-params (`spans`, `hidden`, `accounted`) plus a
//! fourth WP2 added (`tables`) — the repo bans
//! `#[allow(clippy::too_many_arguments)]`.

use super::style::{
    StyleCtx, blockquote_scope, code_fence_scope, frontmatter_scope, heading_style, hr_scope,
    list_marker_style, verbatim_style,
};
use super::table::emit_table;
use super::walk_inline::emit_inlines;
use super::{EmitOut, hide_range, line_local, push_span_split_by_line};
use crate::element::block::{Block, CodeFenceM, FrontmatterM, ListItemM};
use crate::parse::line_at;
use rune_core::assert_invariant;
use rune_syntax::element::{ByteRange, RevealState};
use rune_syntax::{ScopeId, SyntaxSpan};

/// A block whose body lines sit between an opening and a closing delimiter
/// line, ready to be emitted Revealed. A fence paints its delimiters at the
/// same scope as its body; frontmatter dims its `---` lines instead, so the
/// two scopes are named apart.
struct DelimitedRevealed<'a> {
    open: ByteRange,
    content_lines: &'a [ByteRange],
    close: Option<ByteRange>,
    delimiter_scope: ScopeId,
    body_scope: ScopeId,
}

/// Every piece here (`open`, each of `content_lines`, `close`) is already
/// exactly one physical line's range, computed container-aware at parse
/// time — never re-derived from the block's whole range as one contiguous
/// multi-line span. That whole range spans every line the block occupies
/// INCLUDING any repeating container prefix (blockquote's `"> "`) on
/// continuation lines; pushing it whole through the generic
/// per-physical-line splitter (as this path used to) re-hides/re-shows
/// those container-prefix bytes a second time, on top of whatever the
/// container itself already claimed for that line.
fn emit_delimited_revealed(
    content: &str,
    starts: &[usize],
    block: DelimitedRevealed<'_>,
    out: &mut EmitOut,
) {
    let push = |range: ByteRange, scope: ScopeId, out: &mut EmitOut| {
        push_span_split_by_line(content, starts, range, scope, RevealState::Revealed, out);
    };
    push(block.open, block.delimiter_scope, out);
    for &line in block.content_lines {
        push(line, block.body_scope, out);
    }
    if let Some(close) = block.close {
        push(close, block.delimiter_scope, out);
    }
}

fn emit_code_fence(content: &str, starts: &[usize], cf: &CodeFenceM, out: &mut EmitOut) {
    if cf.sm.state() == RevealState::Revealed {
        emit_delimited_revealed(
            content,
            starts,
            DelimitedRevealed {
                open: cf.fence_open,
                content_lines: &cf.content_lines,
                close: cf.fence_close,
                delimiter_scope: code_fence_scope(),
                body_scope: code_fence_scope(),
            },
            out,
        );
        return;
    }
    // Rendered: the delimiter lines leave the display entirely and the body
    // stays. Each is hidden or pushed one physical line at a time, so a
    // container's own repeating prefix in the gaps between them is never
    // claimed a second time.
    hide_range(content, starts, cf.fence_open, out);
    if let Some(close) = cf.fence_close {
        hide_range(content, starts, close, out);
    }
    for &line in &cf.content_lines {
        push_span_split_by_line(
            content,
            starts,
            line,
            code_fence_scope(),
            RevealState::Rendered,
            out,
        );
    }
}

fn emit_frontmatter(content: &str, starts: &[usize], fm: &FrontmatterM, out: &mut EmitOut) {
    emit_delimited_revealed(
        content,
        starts,
        DelimitedRevealed {
            open: fm.open,
            content_lines: &fm.content_lines,
            close: fm.close,
            delimiter_scope: frontmatter_scope(),
            body_scope: code_fence_scope(),
        },
        out,
    );
}

/// True when `item`'s decor-suppression case applies: its first child is a
/// heading whose own line is the item marker's line. That heading already
/// paints an icon on the row (`push_heading_decor`), so the bullet decor
/// this item would otherwise contribute (`push_list_marker_decor`) must be
/// skipped rather than stacked onto the same `LineDecor`. A heading that is
/// a later child, or one that starts on a different row (a blank line, or
/// any block, between the marker and it), leaves the bullet untouched.
fn leads_with_own_line_heading(item: &ListItemM) -> bool {
    matches!(
        item.children.first(),
        Some(Block::Heading(h)) if h.line == item.line
    )
}

fn emit_list_item(
    content: &str,
    starts: &[usize],
    item: &ListItemM,
    ordered: bool,
    depth: u8,
    out: &mut EmitOut,
) {
    if item.sm.state() == RevealState::Revealed {
        push_span_split_by_line(
            content,
            starts,
            item.marker,
            list_marker_style(item.task.is_some()),
            RevealState::Revealed,
            out,
        );
    } else if let Some(task) = item.task {
        // A task item's checkbox substitutes to a glyph even
        // while concealed — plain bullet/ordered markers (the `else` arm
        // below) stay fully hidden, a deliberate "list markers are always
        // concealed" divergence, not a bug.
        // The "- "/"1. " prefix before the checkbox is hidden exactly like
        // a plain marker; only the checkbox itself substitutes.
        let before = ByteRange::new(item.marker.start, task.start);
        hide_range(content, starts, before, out);
        push_task_checkbox(content, starts, task, out);
        // Whatever sits between the checkbox and the item's own content
        // (normally exactly one space) is deliberately left UNCLAIMED here:
        // `fill_gaps` (`emit/mod.rs`) supplies it verbatim as an ordinary
        // `Identical` span, so 0/1/N trailing spaces round-trip exactly —
        // see `push_task_checkbox`'s docs for why this keeps the
        // substitution byte-length-neutral against `SyntaxSnapshot`'s
        // hidden-ranges-only coordinate model instead of hand-rolling a
        // second hidden-range delta for the difference.
    } else {
        hide_range(content, starts, item.marker, out);
        // Task items keep their `☐`/`☑` checkbox substitution (the `if let
        // Some(task)` arm above) and get NO bullet decor on top of it — the
        // checkbox already communicates the marker.
        if !leads_with_own_line_heading(item) {
            let line = line_at(starts, item.marker.start);
            let marker_text = content
                .get(item.marker.start..item.marker.end)
                .unwrap_or("");
            super::decor::push_list_marker_decor(out, line, ordered, depth, marker_text);
        }
    }
    for c in &item.children {
        // Saturating: a pathological ~256-deep nested list must degrade to a
        // repeated bullet glyph, never overflow a u8 — an overflow panic in
        // a debug/fuzz build would lose the unsaved buffer.
        emit_block(content, starts, c, depth.saturating_add(1), out);
    }
}

/// Substitutes a task item's `"[ ]"`/`"[x]"`/`"[X]"` — always exactly 3
/// bytes (`ListItemM::task`'s docs) — with its checkbox glyph: `☐`
/// (U+2610) unchecked, `☑` (U+2611) checked.
///
/// Deliberately NOT routed through `hide_range` — this substitutes visible
/// content, it doesn't hide it — and deliberately NOT built via
/// `push_span_split_by_line` (which only ever copies `content[range]`
/// itself into `Substituted::text`, never a genuinely different string).
/// Routes the claim itself through `EmitOut::claim_whole` — there is no
/// half-glyph substitution, so an overlap refuses the whole claim instead
/// of drawing over bytes another producer already owns.
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
fn push_task_checkbox(content: &str, starts: &[usize], task: ByteRange, out: &mut EmitOut) {
    let Some(bytes) = content.get(task.start..task.end) else {
        return;
    };
    assert_invariant!(task.len() == 3, || {
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
        hide_range(content, starts, task, out);
        return;
    }
    let checked = bytes.as_bytes().get(1).is_some_and(|&b| b != b' ');
    let glyph = if checked { "\u{2611}" } else { "\u{2610}" };
    let line = line_at(starts, task.start);

    let Some(ll) = line_local(content.len(), starts, line, task.start..task.end) else {
        assert_invariant!(false, || {
            format!(
                "task checkbox range [{},{}) escaped line {line}'s own physical bounds — producer bug",
                task.start, task.end
            )
        });
        return;
    };
    let Ok(granted) = out.claim_whole(ll) else {
        return;
    };

    let span = SyntaxSpan::substituted_mapped(
        // Pre-WP4 this was `StyleId::TaskMarker`; WP4 folded that variant
        // into `list_marker_style`'s task arm ("markup.list.checked").
        list_marker_style(true),
        glyph.to_string(),
        task.start..task.end,
        vec![u32::try_from(task.start).ok()],
    );
    granted.push_visible(vec![span]);
}

pub(crate) fn emit_block(
    content: &str,
    starts: &[usize],
    block: &Block,
    depth: u8,
    out: &mut EmitOut,
) {
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
                        out,
                    );
                }
            } else {
                hide_range(content, starts, h.marker, out);
                emit_inlines(
                    content,
                    starts,
                    &h.inlines,
                    StyleCtx::Override(heading_style(h.level)),
                    out,
                );
                super::decor::push_heading_decor(out, h.line, h.level);
                // A setext heading's underline row is claimed here, never
                // left for `fill_gaps` — its bytes are hidden exactly like
                // the ATX marker, and the freed row carries a full-width
                // rule in the heading's own style, not the thematic-break
                // style (user-decided target behavior).
                if let Some(underline) = h.underline {
                    hide_range(content, starts, underline, out);
                    let underline_line = line_at(starts, underline.start);
                    super::decor::push_heading_rule_decor(out, underline_line, h.level);
                }
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
                    );
                } else {
                    hide_range(content, starts, m.marker, out);
                    super::decor::push_quote_marker_decor(out, m.line);
                }
            }
            for c in &bq.children {
                emit_block(content, starts, c, depth, out);
            }
        }
        Block::CodeFence(cf) => emit_code_fence(content, starts, cf, out),
        Block::List(list) => {
            for item in &list.items {
                emit_list_item(content, starts, item, list.ordered, depth, out);
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
                );
            } else {
                hide_range(content, starts, hr.range, out);
                super::decor::push_hr_decor(out, hr.line);
            }
        }
        Block::Frontmatter(fm) => emit_frontmatter(content, starts, fm, out),
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
                );
            }
        }
        Block::Table(t) => emit_table(content, starts, t, out),
    }
}
