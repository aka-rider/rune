//! AST -> `Inline` construction: dispatch (`build_inline`) plus the
//! delimiter-gap derivations (`child_gap_delims`, `link_url_range`,
//! `wikilink_label_range`) that recover markup ranges comrak has no
//! dedicated node for. `![alt](url)`'s own alt/target range derivation and
//! `![[target]]` embed recovery live in the sibling `embed` module — split
//! out to keep this one under the 500-line budget.

use super::embed::{image_alt_range, image_target_range, recover_embeds};
use super::{ScanHint, line_end_at, node_range};
use crate::element::inline::{
    EmphasisKind, EmphasisM, ImageM, Inline, InlineCodeM, LinkM, TextRun, WikiLinkM,
};
use comrak::nodes::{AstNode, NodeValue};
use rune_syntax::element::{ByteRange, RevealSm, RevealState};

/// Delimiter ranges derived from the gap between a node's range and its
/// first/last child's range (plan Context "Parse": "Delimiter ranges ...
/// are derived from the gap between a node's range and its first/last
/// child's range"). A childless node (empty emphasis/strong — unusual, but
/// not impossible with unbalanced markup the parser still recovers into a
/// node) has no inner content to bound "open" and "close" against on
/// opposite sides, so `open` covers the WHOLE range and `close` is a
/// zero-length range at its end — contributing exactly one hidden run when
/// concealed, instead of the whole range being hidden twice (once as each
/// fallback independently defaulted to the full span).
fn child_gap_delims(
    content: &str,
    starts: &[usize],
    node: &AstNode,
    range: ByteRange,
) -> (ByteRange, ByteRange) {
    match (node.first_child(), node.last_child()) {
        (Some(first), Some(last)) => {
            let open_end = node_range(content, starts, first)
                .start
                .max(range.start)
                .min(range.end);
            let close_start = node_range(content, starts, last)
                .end
                .max(range.start)
                .min(range.end);
            (
                ByteRange::new(range.start, open_end),
                ByteRange::new(close_start, range.end),
            )
        }
        _ => (range, ByteRange::new(range.end, range.end)),
    }
}

/// True if `node` itself is a WikiLink whose own match spans a raw newline,
/// OR any node in its descendant subtree is — i.e. whether comrak's
/// internal line-counter desync (see `build_inlines`'s docs) could have
/// touched anything inside `node`. A PARENT that merely WRAPS such a
/// wikilink (`"*[[\n]]\n(*"` — an Emphasis whose child list is
/// `[WikiLink, Text]`) is just as exposed as the wikilink itself: its own
/// `child_gap_delims` reads the LAST child's sourcepos to place the close
/// delimiter, and that sibling's sourcepos is exactly what the desync
/// corrupts (verification round 4's MAJOR — the wrapper's close `*` got
/// recorded hidden on the wikilink's own line while the emitter placed it,
/// unhidden, on the actual closing line: a coverage/duplicate-claim bug at
/// a new site, same root cause).
///
/// Checks for both `'\n'` and `'\r'`, not just `'\n'`: a wikilink match
/// embedding either desyncs comrak's own internal line counter for the
/// rest of the paragraph, the same way an embedded `\n` does — checking
/// only `'\n'` would let a `\r`-embedding match through undetected,
/// leaving a corrupted sibling's range to collide with an already-claimed
/// byte under strict invariants.
fn subtree_has_multiline_wikilink<'a>(
    content: &str,
    starts: &[usize],
    node: &'a AstNode<'a>,
) -> bool {
    if matches!(node.data.borrow().value, NodeValue::WikiLink(_)) {
        let range = node_range(content, starts, node);
        if content
            .get(range.start..range.end)
            .is_none_or(|s| s.contains(['\n', '\r']))
        {
            return true;
        }
    }
    node.children()
        .any(|child| subtree_has_multiline_wikilink(content, starts, child))
}

pub(super) fn build_inlines<'a>(
    content: &str,
    starts: &[usize],
    parent: &'a AstNode<'a>,
    hint: &ScanHint,
) -> Vec<Inline> {
    let mut out = Vec::new();
    for child in parent.children() {
        // RESIDUAL PRODUCER fix (verification rounds 3-4, "[[\n]]"): a
        // WikiLink match that embeds a raw newline desyncs comrak's OWN
        // internal line counter for the REST of this paragraph — verified
        // empirically: a plain-text sibling several nodes later reported
        // the wrong physical line, so its sourcepos-derived byte range
        // came out shifted earlier than its true position, landing on
        // top of an EARLIER sibling's already-claimed bytes (a
        // producer-bug duplicate-claim under strict invariants). Round 4
        // found the SAME corruption reaching a PARENT wrapping such a
        // wikilink too (`"*[[\n]]\n(*"`): an Emphasis/Strikethrough/Link's
        // own `child_gap_delims`/`build_inlines` calls read a LAST
        // child's (possibly corrupted) sourcepos, so trusting the
        // wrapper's internal structure is just as unsafe as trusting a
        // corrupted sibling directly — `subtree_has_multiline_wikilink`
        // checks the WHOLE subtree, not just "is this child itself the
        // wikilink". Reconstructing a corrupted node's or its ancestor's
        // TRUE internal structure from comrak's now-unreliable sourcepos
        // is not generally possible — but this crate already has a
        // reliable, comrak-sourcepos-INDEPENDENT way to find where each
        // remaining physical line's OWN content starts: `hint`, the same
        // per-line container-prefix scan `blockquote_markers`/a fenced
        // code block's `content_lines` already use to skip a repeating
        // `"> "` (or a list item's continuation indent) on a line
        // comrak's sourcepos alone can't be trusted for either. So the
        // recovery is per PHYSICAL LINE (never one contiguous span, which
        // could reach into a LATER line's container-marker bytes the
        // block scanner independently — and reliably — already claims):
        // the corrupted child's own OUTER range gives us its reliable
        // START (the first line's own true content start, verified
        // reliable even for a wrapper's own delimiter-matched sourcepos —
        // that's assigned by the base emphasis/strong pass, BEFORE the
        // wikilink substitution that corrupts its children, so only its
        // INTERNAL structure is untrustworthy, not its own outer start).
        // But when that node's own range spans MULTIPLE physical lines
        // itself — nested in a container, a repeated "> " sits on every
        // CONTINUATION line too, bytes `blockquote_markers` independently
        // (and reliably) already claims — so every line past the FIRST,
        // including the node's own remaining lines, is rebuilt the same
        // `hint`-derived way as the lines strictly after it: never one
        // blind contiguous span, whether or not it happens to still be
        // "inside" the corrupted node's own reported extent.
        let range = node_range(content, starts, child);
        if subtree_has_multiline_wikilink(content, starts, child) {
            // This rebuild iterates and clamps by `starts` — the same
            // index `ScanHint`'s own marker_ends map is keyed by (see the
            // `BlockQuote` arm in `parse::block`) — to reconstruct content
            // per physical line, mirroring the fence/heading content-line
            // fix (`parse::block`'s `CodeBlock` arm).
            let parent_range = node_range(content, starts, parent);
            let parent_last_line = super::line_at(
                starts,
                parent_range.end.saturating_sub(1).max(parent_range.start),
            );
            let first_line = super::line_at(starts, range.start);
            let first_line_end = line_end_at(content.len(), starts, first_line)
                .min(range.end)
                .min(parent_range.end)
                .max(range.start);
            // Each piece pushed below is already clamped to exactly ONE
            // comrak line's own extent, so `content_lines` is trivially
            // `vec![range]` for every one of them — no need to re-derive
            // via `per_line_content`.
            let first_range = ByteRange::new(range.start, first_line_end).clamp(content.len());
            out.push(Inline::Text(TextRun {
                range: first_range,
                content_lines: vec![first_range],
            }));
            for line in (first_line + 1)..=parent_last_line {
                let s = hint.start_for_line(starts, line);
                // CLASS A fallout (verification round 5): a lone `\r`
                // elsewhere in this SAME buffer line can make comrak
                // split its OWN block-level parsing at that point — this
                // paragraph's true (comrak-correct) extent can end
                // MID-buffer-line, with a SEPARATE sibling block (e.g. a
                // blockquote comrak recognizes starting right after the
                // `\r`) independently claiming the rest of that buffer
                // line. Blindly rebuilding the WHOLE buffer line here
                // (as if it were entirely this paragraph's own trailing
                // content) would re-claim bytes that sibling block also
                // claims — clamp to `parent_range.end`, this paragraph's
                // own reliable outer bound, same as the first piece
                // above.
                let e = line_end_at(content.len(), starts, line).min(parent_range.end);
                if s < e {
                    let piece_range = ByteRange::new(s, e).clamp(content.len());
                    out.push(Inline::Text(TextRun {
                        range: piece_range,
                        content_lines: vec![piece_range],
                    }));
                }
            }
            return recover_embeds(content, starts, out);
        }
        out.push(build_inline(content, starts, child, hint));
    }
    recover_embeds(content, starts, out)
}

enum InlineKind {
    TextLike,
    Emph,
    Strong,
    Strikethrough,
    Code(usize),
    Link(String),
    /// `![alt](url)` markdown image syntax — carries comrak's own decoded
    /// URL, same shape as `Link`'s. `![[target]]` never reaches here: it
    /// has no dedicated AST node at all (see `recover_embeds`'s docs) and
    /// is recovered separately, as a post-pass over the flattened `Text`
    /// runs this same dispatch produces for ordinary prose.
    Image(String),
    WikiLink(String),
}

/// The leading run of exactly `want` backtick bytes at the start of
/// `line` — comrak's own delimiter matching guarantees a code span's
/// OPEN is always found this way (the backtick run that started the
/// match), so `line.start` is trustworthy as-is; this only clamps
/// defensively (never scans backward past `line.start`) for the same
/// "checked, not assumed" reason `trailing_backtick_run` below scans at
/// all. Returns `line` itself if fewer than `want` backtick bytes are
/// actually present (should not happen; degrades to "whole line hidden"
/// rather than panicking on a malformed slice).
fn leading_backtick_run(content: &str, line: ByteRange, want: usize) -> ByteRange {
    let bytes = content.as_bytes();
    let mut end = line.start;
    while end < line.end && end - line.start < want && bytes.get(end) == Some(&b'`') {
        end += 1;
    }
    if end - line.start == want {
        ByteRange::new(line.start, end)
    } else {
        line
    }
}

/// The trailing run of exactly `want` backtick bytes at the end of
/// `line`, found by scanning BACKWARD from `line.end` rather than
/// subtracting `want` from `line.end` directly (verification round 9's
/// exhaustive audit, third pass): comrak's own sourcepos for a `Code`
/// node's outer span can extend one or more bytes past the true closing
/// backtick run — confirmed empirically with two independent minimal
/// repros (`"]\n x```\n``` `\"`, where a trailing space after the real
/// close run got folded into the node's own end column; and `">t\n>`\n`>"`,
/// where the very next character after a single-backtick close got
/// folded in the same way). Neither involves a container prefix or a
/// `\r` — this is a raw comrak sourcepos quirk, not a mixed-index seam,
/// so the fix is structural: don't trust `line.end` as the close run's
/// own end, DERIVE it by skipping any trailing non-backtick bytes first.
fn trailing_backtick_run(content: &str, line: ByteRange, want: usize) -> ByteRange {
    let bytes = content.as_bytes();
    let mut end = line.end;
    while end > line.start && bytes.get(end - 1) != Some(&b'`') {
        end -= 1;
    }
    let mut start = end;
    while start > line.start && bytes.get(start - 1) == Some(&b'`') {
        start -= 1;
    }
    if end - start >= want {
        let close_start = end - want;
        ByteRange::new(close_start, end)
    } else {
        // No backtick run of the expected length found in this line at
        // all — fall back to the naive clamp rather than guessing.
        let close_start = line.end.saturating_sub(want).max(line.start);
        ByteRange::new(close_start, line.end)
    }
}

fn inline_kind(v: &NodeValue) -> InlineKind {
    match v {
        NodeValue::Emph => InlineKind::Emph,
        NodeValue::Strong => InlineKind::Strong,
        NodeValue::Strikethrough => InlineKind::Strikethrough,
        NodeValue::Code(c) => InlineKind::Code(c.num_backticks),
        NodeValue::Link(l) => InlineKind::Link(l.url.clone()),
        NodeValue::Image(l) => InlineKind::Image(l.url.clone()),
        NodeValue::WikiLink(w) => InlineKind::WikiLink(w.url.clone()),
        // Text, SoftBreak, LineBreak, HtmlInline, and any other inline node
        // kind this crate doesn't model degrade to plain text
        // ("unknown syntax degrades to visible raw text, never lost").
        _ => InlineKind::TextLike,
    }
}

fn build_inline<'a>(
    content: &str,
    starts: &[usize],
    node: &'a AstNode<'a>,
    hint: &ScanHint,
) -> Inline {
    let range = node_range(content, starts, node);
    // BUFFER line — see `parse::block::build_block`'s docs on why `line`
    // is derived from the already-correct byte range, not comrak's raw
    // line number (verification round 5 CLASS A).
    let line = super::line_at(starts, range.start);
    let kind = inline_kind(&node.data.borrow().value);

    match kind {
        InlineKind::TextLike => Inline::Text(TextRun {
            range,
            // MAJOR fix (verification round 9's exhaustive audit): an
            // unmodeled inline node (raw HTML, a hard line break, ...)
            // can legitimately span multiple physical lines — verified
            // empirically with a multi-line `<span\n...>` HTML tag
            // nested in a blockquote, which used to re-claim the second
            // line's own "> " marker the same way an un-fixed fence or
            // table did. `per_line_content` naturally collapses to
            // `vec![range]` for the overwhelmingly common single-line
            // case.
            content_lines: super::per_line_content(content, starts, range, hint),
        }),
        InlineKind::Image(url) => {
            let alt = image_alt_range(content, starts, node, range);
            let target = image_target_range(alt.end, range, &url, content.len());
            Inline::Image(ImageM {
                sm: RevealSm::new(RevealState::Rendered),
                range,
                alt,
                target,
                target_text: url,
                is_wikilink: false,
                line,
                content_lines: super::per_line_content(content, starts, range, hint),
            })
        }
        InlineKind::Emph => {
            let (open, close) = child_gap_delims(content, starts, node, range);
            let children = build_inlines(content, starts, node, hint);
            Inline::Emphasis(EmphasisM {
                sm: RevealSm::new(RevealState::Rendered),
                kind: EmphasisKind::Italic,
                range,
                open,
                close,
                children,
                line,
                content_lines: super::per_line_content(content, starts, range, hint),
            })
        }
        InlineKind::Strong => {
            let (open, close) = child_gap_delims(content, starts, node, range);
            let children = build_inlines(content, starts, node, hint);
            Inline::Emphasis(EmphasisM {
                sm: RevealSm::new(RevealState::Rendered),
                kind: EmphasisKind::Bold,
                range,
                open,
                close,
                children,
                line,
                content_lines: super::per_line_content(content, starts, range, hint),
            })
        }
        InlineKind::Strikethrough => {
            let (open, close) = child_gap_delims(content, starts, node, range);
            let children = build_inlines(content, starts, node, hint);
            Inline::Emphasis(EmphasisM {
                sm: RevealSm::new(RevealState::Rendered),
                kind: EmphasisKind::Strike,
                range,
                open,
                close,
                children,
                line,
                content_lines: super::per_line_content(content, starts, range, hint),
            })
        }
        InlineKind::Code(num_backticks) => {
            // MAJOR fix (verification round 9's exhaustive audit, second
            // pass): a backtick run can never itself contain a newline, so
            // `open`/`close` are each genuinely single-line — but deriving
            // them via raw arithmetic straight off `range.start`/`range.end`
            // silently assumed `range` is CONTIGUOUS raw bytes, which only
            // holds when every line the span crosses has the SAME
            // container-marker width. A blockquote's lazy-continuation line
            // (no "> " at all) followed by one with a bare ">" (no trailing
            // space, 1 byte instead of 2) breaks that assumption:
            // `range.end - num_backticks` lands mid-marker instead of on
            // the real closing backticks, producing a `close` that
            // overlaps the marker's own hidden range. Deriving `open` from
            // the FIRST content_lines piece and `close` from the LAST one
            // makes both track the actual per-line content instead of raw
            // extents, closing the same class VerbatimM/EmphasisM's own
            // `range` had.
            let content_lines = super::per_line_content(content, starts, range, hint);
            let first_line = content_lines.first().copied().unwrap_or(range);
            let last_line = content_lines.last().copied().unwrap_or(range);
            let open = leading_backtick_run(content, first_line, num_backticks);
            let close = trailing_backtick_run(content, last_line, num_backticks);
            let content_range = ByteRange::new(open.end, close.start).clamp(content.len());
            Inline::Code(InlineCodeM {
                sm: RevealSm::new(RevealState::Rendered),
                range,
                open,
                close,
                content: content_range,
                line,
                content_lines,
                inner_lines: super::per_line_content(content, starts, content_range, hint),
            })
        }
        InlineKind::Link(url) => {
            let text = build_inlines(content, starts, node, hint);
            let url_range = link_url_range(range, &text, &url, content.len());
            Inline::Link(LinkM {
                sm: RevealSm::new(RevealState::Rendered),
                content_lines: super::per_line_content(content, starts, range, hint),
                range,
                line,
                text,
                url,
                url_range,
            })
        }
        InlineKind::WikiLink(target) => {
            // `build_inlines` (the only caller) already filters out any
            // WikiLink whose own match spans a raw newline before ever
            // reaching here (see its docs) — this arm only ever builds a
            // genuinely single-line wikilink, so `range` is safe to hand
            // straight to the byte-arithmetic label derivation below.
            let label = wikilink_label_range(content, range);
            Inline::WikiLink(WikiLinkM {
                sm: RevealSm::new(RevealState::Rendered),
                range,
                line,
                target,
                label,
            })
        }
    }
}

/// `LinkM::url_range` from delimiter arithmetic, not string search: a
/// markdown link is always `"[" text "](" url [" \"title\""] ")"`, so the
/// URL begins exactly 2 bytes after the text closes (the `']'` immediately
/// followed by `'('`) — for an empty-text link (`"[](url)"`), the text
/// "closes" at the position right after the opening `'['`. `url.len()`
/// (comrak's own decoded URL length) bounds the far end, so a trailing
/// `" \"title\""` is never swept in. This replaces a `str::rfind` heuristic
/// that could false-match unrelated text inside the token.
fn link_url_range(range: ByteRange, text: &[Inline], url: &str, content_len: usize) -> ByteRange {
    let text_close = text
        .last()
        .map(|c| c.range().end)
        .unwrap_or(range.start.saturating_add(1));
    let url_start = text_close.saturating_add(2).min(range.end).min(content_len);
    let url_end = url_start
        .saturating_add(url.len())
        .min(range.end)
        .min(content_len);
    ByteRange::new(url_start, url_end)
}

/// `[[target|label]]` displays a label: the part after the `'|'` when a pipe
/// is present, else `target` itself — always the byte span between the
/// `"[["`/`"]]"` delimiters (minus any `"target|"` prefix).
///
/// MAJOR fix (verification round 3): this USED to read the label's range off
/// comrak's own child-node sourcepos (`node.first_child()`/`last_child()`),
/// mirroring `child_gap_delims`'s pattern for emphasis/strong. But for a
/// WikiLink specifically, comrak's child Text node sourcepos is unreliable
/// when the target has LEADING WHITESPACE that gets trimmed (`"[[ a]]"` ->
/// url `"a"`): the reported span pointed at the trimmed-away leading space
/// instead of the retained character, undercounting the label by exactly
/// one byte (invisible for a 1-byte ASCII char — "[[ a]]" rendered " "
/// instead of " a" — and OUT-OF-BOUNDS-splitting for a multibyte final char,
/// since the reported end landed mid-char: "[[ 日]]" / "[[ 👍]]").
///
/// A wikilink's own OUTER sourcepos (`range`, already proven reliable by the
/// WP0 sourcepos spike) is always `"[[" target ["|" label] "]]"` — so the
/// label always ends exactly 2 bytes before `range.end` (skipping the ASCII
/// `"]]"`, always a char boundary) and starts either 2 bytes after
/// `range.start` (skipping `"[["`, no pipe) or right after the first `'|'`
/// found between the delimiters. Pure byte arithmetic off `range`'s own
/// boundaries never depends on a child node's sourcepos at all, so this
/// class of bug cannot recur here.
fn wikilink_label_range(content: &str, range: ByteRange) -> ByteRange {
    let inner_start = range.start.saturating_add(2).min(range.end);
    let label_end = range.end.saturating_sub(2).max(inner_start);
    let inner = content.get(inner_start..label_end).unwrap_or("");
    let label_start = match inner.find('|') {
        Some(pipe_offset) => inner_start
            .saturating_add(pipe_offset)
            .saturating_add(1)
            .min(label_end),
        None => inner_start,
    };
    ByteRange::new(label_start, label_end).clamp(content.len())
}
