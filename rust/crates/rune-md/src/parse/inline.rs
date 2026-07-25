//! AST -> `Inline` construction: dispatch (`build_inline`) plus the
//! delimiter-gap derivations (`child_gap_delims`, `link_url_range`,
//! `wikilink_label_range`) that recover markup ranges comrak has no
//! dedicated node for.

use super::node_range;
use crate::element::inline::{
    EmphasisKind, EmphasisM, Inline, InlineCodeM, LinkM, TextRun, WikiLinkM,
};
use crate::element::{ByteRange, RevealSm, RevealState};
use comrak::nodes::{AstNode, NodeValue};

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

pub(super) fn build_inlines<'a>(
    content: &str,
    starts: &[usize],
    parent: &'a AstNode<'a>,
) -> Vec<Inline> {
    let mut out = Vec::new();
    for child in parent.children() {
        out.push(build_inline(content, starts, child));
    }
    out
}

enum InlineKind {
    TextLike,
    Emph,
    Strong,
    Strikethrough,
    Code(usize),
    Link(String),
    /// Phase-1 scope: inline images are plain revealed text runs, no
    /// machine (plan: "Inline images ... -> plain revealed text runs").
    Image,
    WikiLink(String),
}

fn inline_kind(v: &NodeValue) -> InlineKind {
    match v {
        NodeValue::Emph => InlineKind::Emph,
        NodeValue::Strong => InlineKind::Strong,
        NodeValue::Strikethrough => InlineKind::Strikethrough,
        NodeValue::Code(c) => InlineKind::Code(c.num_backticks),
        NodeValue::Link(l) => InlineKind::Link(l.url.clone()),
        NodeValue::Image(_) => InlineKind::Image,
        NodeValue::WikiLink(w) => InlineKind::WikiLink(w.url.clone()),
        // Text, SoftBreak, LineBreak, HtmlInline, and any other inline node
        // kind this crate doesn't model degrade to plain text (plan §0:
        // "unknown syntax degrades to visible raw text, never lost").
        _ => InlineKind::TextLike,
    }
}

fn build_inline<'a>(content: &str, starts: &[usize], node: &'a AstNode<'a>) -> Inline {
    let range = node_range(content, starts, node);
    let sp = node.data.borrow().sourcepos;
    let line = sp.start.line.saturating_sub(1);
    let kind = inline_kind(&node.data.borrow().value);

    match kind {
        InlineKind::TextLike | InlineKind::Image => Inline::Text(TextRun { range }),
        InlineKind::Emph => {
            let (open, close) = child_gap_delims(content, starts, node, range);
            let children = build_inlines(content, starts, node);
            Inline::Emphasis(EmphasisM {
                sm: RevealSm::new(RevealState::Rendered),
                kind: EmphasisKind::Italic,
                range,
                open,
                close,
                children,
                line,
            })
        }
        InlineKind::Strong => {
            let (open, close) = child_gap_delims(content, starts, node, range);
            let children = build_inlines(content, starts, node);
            Inline::Emphasis(EmphasisM {
                sm: RevealSm::new(RevealState::Rendered),
                kind: EmphasisKind::Bold,
                range,
                open,
                close,
                children,
                line,
            })
        }
        InlineKind::Strikethrough => {
            let (open, close) = child_gap_delims(content, starts, node, range);
            let children = build_inlines(content, starts, node);
            Inline::Emphasis(EmphasisM {
                sm: RevealSm::new(RevealState::Rendered),
                kind: EmphasisKind::Strike,
                range,
                open,
                close,
                children,
                line,
            })
        }
        InlineKind::Code(num_backticks) => {
            let open_end = range.start.saturating_add(num_backticks).min(range.end);
            let close_start = range.end.saturating_sub(num_backticks).max(open_end);
            let open = ByteRange::new(range.start, open_end);
            let close = ByteRange::new(close_start, range.end);
            let content_range = ByteRange::new(open.end, close.start);
            Inline::Code(InlineCodeM {
                sm: RevealSm::new(RevealState::Rendered),
                range,
                open,
                close,
                content: content_range,
                line,
            })
        }
        InlineKind::Link(url) => {
            let text = build_inlines(content, starts, node);
            let url_range = link_url_range(range, &text, &url, content.len());
            Inline::Link(LinkM {
                sm: RevealSm::new(RevealState::Rendered),
                range,
                line,
                text,
                url,
                url_range,
            })
        }
        InlineKind::WikiLink(target) => {
            let label = wikilink_label_range(content, starts, node, range);
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

/// `[[target|label]]` has a child (the label text) when a pipe is present;
/// `[[target]]` alone has none, and the label is `target` itself between the
/// `"[["`/`"]]"` delimiters.
fn wikilink_label_range(
    content: &str,
    starts: &[usize],
    node: &AstNode,
    range: ByteRange,
) -> ByteRange {
    if let (Some(first), Some(last)) = (node.first_child(), node.last_child()) {
        let start = node_range(content, starts, first).start;
        let end = node_range(content, starts, last).end;
        return ByteRange::new(start, end).clamp(content.len());
    }
    let inner_start = range.start.saturating_add(2).min(range.end);
    let inner_end = range.end.saturating_sub(2).max(inner_start);
    ByteRange::new(inner_start, inner_end).clamp(content.len())
}
