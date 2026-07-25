//! comrak invocation + AST walk -> `Block`/`Inline` tree (plan Context,
//! "Parse"). Sourcepos -> byte-range conversion is the WP0-proven formula
//! from `tests/spike_sourcepos.rs`, shared here so the spike itself now
//! calls this module instead of keeping its own copy (Ground rule 3: "reuse
//! that conversion").

use crate::element::block::{
    Block, BlockquoteM, BlockquoteMarkerM, CodeFenceM, FrontmatterM, HrM, ListItemM, ListM,
    ParagraphM, VerbatimKind, VerbatimM,
};
use crate::element::inline::{
    EmphasisKind, EmphasisM, Inline, InlineCodeM, LinkM, TextRun, WikiLinkM,
};
use crate::element::{ByteRange, RevealSm, RevealState};
use comrak::nodes::{AstNode, ListType, NodeValue, Sourcepos};
use comrak::{Arena, Options, parse_document};

/// Byte offset of the start of each line — port of
/// `pkg/editor/buffer/lineindex.go:computeLineStarts`, and the index every
/// sourcepos conversion below is built on.
pub fn line_starts(src: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// The WP0-proven conversion: comrak `Sourcepos` (1-based, end-inclusive,
/// UTF-8 byte columns) -> an absolute half-open `ByteRange`.
/// `start = line_starts[l-1] + (c-1)`, `end = line_starts[el-1] + ec`
/// (Gotchas: "comrak sourcepos is 1-based, end-inclusive, byte columns").
/// Never panics on a malformed/out-of-range sourcepos: every lookup goes
/// through `.get()` and falls back to 0 rather than indexing directly.
pub fn sourcepos_to_range(starts: &[usize], sp: Sourcepos) -> ByteRange {
    let start_line = starts
        .get(sp.start.line.saturating_sub(1))
        .copied()
        .unwrap_or(0);
    let end_line = starts
        .get(sp.end.line.saturating_sub(1))
        .copied()
        .unwrap_or(0);
    let start = start_line + sp.start.column.saturating_sub(1);
    let end = end_line + sp.end.column;
    ByteRange::new(start, end)
}

/// The one comrak `Options` value the whole crate parses with — extension
/// set from plan Context ("Parse"). Sourcepos needs no option (Gotchas:
/// "There is no option that enables AST sourcepos").
pub fn options() -> Options<'static> {
    let mut opts = Options::default();
    opts.extension.strikethrough = true;
    opts.extension.tasklist = true;
    opts.extension.table = true;
    opts.extension.wikilinks_title_after_pipe = true;
    opts.extension.front_matter_delimiter = Some("---".to_owned());
    opts
}

pub(crate) fn line_start_at(starts: &[usize], line: usize) -> usize {
    starts.get(line).copied().unwrap_or(0)
}

/// The byte offset of the end of `line`, exclusive of its own trailing
/// `\n` — mirrors `rune_core::buffer::Buffer::line_end`. Shared with
/// `emit.rs`, which splits spans across the same line boundaries.
pub(crate) fn line_end_at(content_len: usize, starts: &[usize], line: usize) -> usize {
    let count = starts.len();
    if line + 1 >= count {
        return content_len;
    }
    starts
        .get(line + 1)
        .copied()
        .unwrap_or(content_len)
        .saturating_sub(1)
        .min(content_len)
}

/// The line index `i` such that `starts[i] <= offset < starts[i+1]` — port
/// of `pkg/editor/buffer/lineindex.go`'s `findLine`. Shared with `emit.rs`.
pub(crate) fn line_at(starts: &[usize], offset: usize) -> usize {
    let idx = starts.partition_point(|&s| s <= offset);
    idx.saturating_sub(1)
}

/// Parse `content` into the top-level `Block` tree. This is the ONLY entry
/// point `DocMachine::sync_content` calls — every downstream module reaches
/// comrak through here.
pub fn parse(content: &str) -> Vec<Block> {
    let starts = line_starts(content);
    let arena = Arena::new();
    let opts = options();
    let root = parse_document(&arena, content, &opts);
    build_blocks(content, &starts, root)
}

fn node_range(content: &str, starts: &[usize], node: &AstNode) -> ByteRange {
    let sp = node.data.borrow().sourcepos;
    sourcepos_to_range(starts, sp).clamp(content.len())
}

/// Delimiter ranges derived from the gap between a node's range and its
/// first/last child's range (plan Context "Parse": "Delimiter ranges ...
/// are derived from the gap between a node's range and its first/last
/// child's range").
fn child_gap_delims(
    content: &str,
    starts: &[usize],
    node: &AstNode,
    range: ByteRange,
) -> (ByteRange, ByteRange) {
    let open_end = node
        .first_child()
        .map(|c| node_range(content, starts, c).start)
        .unwrap_or(range.end)
        .max(range.start)
        .min(range.end);
    let close_start = node
        .last_child()
        .map(|c| node_range(content, starts, c).end)
        .unwrap_or(range.start)
        .max(range.start)
        .min(range.end);
    let open = ByteRange::new(range.start, open_end);
    let close = ByteRange::new(close_start, range.end);
    (open, close)
}

fn build_blocks<'a>(content: &str, starts: &[usize], parent: &'a AstNode<'a>) -> Vec<Block> {
    let mut out = Vec::new();
    for child in parent.children() {
        if let Some(b) = build_block(content, starts, child) {
            out.push(b);
        }
    }
    out
}

fn build_block<'a>(content: &str, starts: &[usize], node: &'a AstNode<'a>) -> Option<Block> {
    let range = node_range(content, starts, node);
    let sp = node.data.borrow().sourcepos;
    let line = sp.start.line.saturating_sub(1);

    let kind = { node.data.borrow().value.clone_kind_tag() };
    match kind {
        BlockKind::Paragraph => {
            let inlines = build_inlines(content, starts, node);
            Some(Block::Paragraph(ParagraphM { range, inlines }))
        }
        BlockKind::Heading(level) => {
            let marker_end = node
                .first_child()
                .map(|c| node_range(content, starts, c).start)
                .unwrap_or(range.end)
                .max(range.start)
                .min(range.end);
            let marker = ByteRange::new(range.start, marker_end);
            let inlines = build_inlines(content, starts, node);
            Some(Block::Heading(crate::element::block::HeadingM {
                sm: RevealSm::new(RevealState::Rendered),
                level,
                line,
                range,
                marker,
                inlines,
            }))
        }
        BlockKind::BlockQuote => {
            let markers = blockquote_markers(content, starts, range);
            let children = build_blocks(content, starts, node);
            Some(Block::Blockquote(BlockquoteM {
                range,
                markers,
                children,
            }))
        }
        BlockKind::CodeBlock { fenced, info } => {
            if !fenced {
                return Some(Block::Verbatim(VerbatimM {
                    sm: RevealSm::new(RevealState::Revealed),
                    range,
                    kind: VerbatimKind::Unknown,
                }));
            }
            let first_line = line;
            let last_line = sp.end.line.saturating_sub(1);
            let first_line_start = line_start_at(starts, first_line);
            let first_line_end = line_end_at(content.len(), starts, first_line);
            let fence_open =
                Some(ByteRange::new(first_line_start, first_line_end).clamp(content.len()));

            let (fence_close, content_range) = if last_line > first_line {
                let ls = line_start_at(starts, last_line);
                let le = line_end_at(content.len(), starts, last_line);
                let close = ByteRange::new(ls, le).clamp(content.len());
                let body =
                    ByteRange::new(line_start_at(starts, first_line + 1), ls).clamp(content.len());
                (Some(close), body)
            } else {
                (None, ByteRange::new(range.end, range.end))
            };

            Some(Block::CodeFence(CodeFenceM {
                sm: RevealSm::new(RevealState::Rendered),
                range,
                first_line,
                last_line,
                language: info,
                fence_open,
                fence_close,
                content: content_range,
            }))
        }
        BlockKind::ThematicBreak => Some(Block::ThematicBreak(HrM {
            sm: RevealSm::new(RevealState::Rendered),
            line,
            range,
        })),
        BlockKind::List => {
            let ordered = matches!(
                node.data.borrow().value,
                NodeValue::List(ref l) if matches!(l.list_type, ListType::Ordered)
            );
            let items = build_list_items(content, starts, node);
            Some(Block::List(ListM { ordered, items }))
        }
        BlockKind::FrontMatter => Some(Block::Frontmatter(FrontmatterM {
            sm: RevealSm::new(RevealState::Revealed),
            range,
        })),
        BlockKind::Table => Some(Block::Verbatim(VerbatimM {
            sm: RevealSm::new(RevealState::Revealed),
            range,
            kind: VerbatimKind::Table,
        })),
        BlockKind::HtmlBlock => Some(Block::Verbatim(VerbatimM {
            sm: RevealSm::new(RevealState::Revealed),
            range,
            kind: VerbatimKind::Html,
        })),
        BlockKind::Document => None,
        BlockKind::Other => Some(Block::Verbatim(VerbatimM {
            sm: RevealSm::new(RevealState::Revealed),
            range,
            kind: VerbatimKind::Unknown,
        })),
    }
}

/// A block-node dispatch key extracted from `NodeValue` up front, so the
/// borrow on `node.data` can be dropped before any recursive call re-borrows
/// the same arena (comrak's `Ast` is a `RefCell`; a live borrow across a
/// recursive `build_blocks`/`build_inlines` call would panic at runtime).
enum BlockKind {
    Document,
    Paragraph,
    Heading(u8),
    BlockQuote,
    CodeBlock { fenced: bool, info: String },
    ThematicBreak,
    List,
    FrontMatter,
    Table,
    HtmlBlock,
    Other,
}

trait ClassifyBlock {
    fn clone_kind_tag(&self) -> BlockKind;
}

impl ClassifyBlock for NodeValue {
    fn clone_kind_tag(&self) -> BlockKind {
        match self {
            NodeValue::Document => BlockKind::Document,
            NodeValue::Paragraph => BlockKind::Paragraph,
            NodeValue::Heading(h) => BlockKind::Heading(h.level),
            NodeValue::BlockQuote => BlockKind::BlockQuote,
            NodeValue::CodeBlock(cb) => BlockKind::CodeBlock {
                fenced: cb.fenced,
                info: cb.info.clone(),
            },
            NodeValue::ThematicBreak => BlockKind::ThematicBreak,
            NodeValue::List(_) => BlockKind::List,
            NodeValue::FrontMatter(_) => BlockKind::FrontMatter,
            NodeValue::Table(_) => BlockKind::Table,
            NodeValue::HtmlBlock(_) => BlockKind::HtmlBlock,
            _ => BlockKind::Other,
        }
    }
}

fn build_list_items<'a>(
    content: &str,
    starts: &[usize],
    list_node: &'a AstNode<'a>,
) -> Vec<ListItemM> {
    let mut items = Vec::new();
    for item_node in list_node.children() {
        let range = node_range(content, starts, item_node);
        let sp = item_node.data.borrow().sourcepos;
        let line = sp.start.line.saturating_sub(1);

        let task = match &item_node.data.borrow().value {
            NodeValue::TaskItem(t) => {
                let sym = sourcepos_to_range(starts, t.symbol_sourcepos);
                let bracket_start = sym.start.saturating_sub(1);
                let bracket_end = sym.end.saturating_add(1).min(content.len());
                Some(ByteRange::new(bracket_start, bracket_end).clamp(content.len()))
            }
            _ => None,
        };

        let marker_end = item_node
            .first_child()
            .map(|c| node_range(content, starts, c).start)
            .unwrap_or(range.end)
            .max(range.start)
            .min(range.end);
        let marker = ByteRange::new(range.start, marker_end);
        let children = build_blocks(content, starts, item_node);

        items.push(ListItemM {
            sm: RevealSm::new(RevealState::Rendered),
            line,
            marker,
            task,
            children,
        });
    }
    items
}

/// Derives one `"> "` marker range per source line covered by a blockquote's
/// range — there is no dedicated comrak node for the marker itself, so this
/// scans the raw text (plan Context "Parse": unmodeled delimiters are
/// derived, never invented from thin air).
fn blockquote_markers(content: &str, starts: &[usize], range: ByteRange) -> Vec<BlockquoteMarkerM> {
    let first_line = line_at(starts, range.start);
    let last_line = line_at(starts, range.end.saturating_sub(1).max(range.start));
    let mut markers = Vec::new();
    for line in first_line..=last_line {
        let line_start = line_start_at(starts, line);
        let line_end = line_end_at(content.len(), starts, line);
        let Some(line_text) = content.get(line_start..line_end.max(line_start)) else {
            continue;
        };
        let trimmed = line_text.trim_start();
        let ws_len = line_text.len() - trimmed.len();
        if let Some(rest) = trimmed.strip_prefix('>') {
            let mut marker_len = ws_len + 1;
            if rest.starts_with(' ') {
                marker_len += 1;
            }
            let marker_end = line_start.saturating_add(marker_len).min(content.len());
            markers.push(BlockquoteMarkerM {
                sm: RevealSm::new(RevealState::Rendered),
                line,
                marker: ByteRange::new(line_start, marker_end),
            });
        }
    }
    markers
}

fn build_inlines<'a>(content: &str, starts: &[usize], parent: &'a AstNode<'a>) -> Vec<Inline> {
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
            let url_range = find_sub_range(content, range, &url);
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

/// Locates `needle` within `range` of `content` and returns its absolute
/// byte range, or an empty range at `range.end` if it can't be found (e.g. a
/// percent-encoded URL that doesn't literally appear in the source). Used
/// for `LinkM::url_range` — Phase-1 doesn't follow links, so an imprecise
/// fallback here never affects reveal/conceal correctness, only click
/// targeting (out of scope this phase).
fn find_sub_range(content: &str, range: ByteRange, needle: &str) -> ByteRange {
    if needle.is_empty() {
        return ByteRange::new(range.end, range.end);
    }
    let Some(full) = content.get(range.start..range.end) else {
        return ByteRange::new(range.end, range.end);
    };
    match full.rfind(needle) {
        Some(pos) => {
            let start = range.start + pos;
            ByteRange::new(start, start + needle.len())
        }
        None => ByteRange::new(range.end, range.end),
    }
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::element::inline::Inline;

    fn text_of(content: &str, r: ByteRange) -> &str {
        content.get(r.start..r.end).unwrap()
    }

    #[test]
    fn heading_marker_and_text_are_byte_exact() {
        let content = "## heading\n";
        let blocks = parse(content);
        assert_eq!(blocks.len(), 1);
        let Block::Heading(h) = &blocks[0] else {
            panic!("expected heading");
        };
        assert_eq!(h.level, 2);
        assert_eq!(text_of(content, h.marker), "## ");
        assert_eq!(text_of(content, h.range), "## heading");
    }

    #[test]
    fn bold_delimiters_and_nested_link() {
        let content = "**[bo*ld*](url)**\n";
        let blocks = parse(content);
        let Block::Paragraph(p) = &blocks[0] else {
            panic!("expected paragraph");
        };
        let Inline::Emphasis(bold) = &p.inlines[0] else {
            panic!("expected bold emphasis");
        };
        assert_eq!(bold.kind, EmphasisKind::Bold);
        assert_eq!(text_of(content, bold.open), "**");
        assert_eq!(text_of(content, bold.close), "**");
        let Inline::Link(link) = &bold.children[0] else {
            panic!("expected link");
        };
        assert_eq!(link.url, "url");
        assert_eq!(text_of(content, link.range), "[bo*ld*](url)");
    }

    #[test]
    fn fenced_code_block_fences_and_content() {
        let content = "```rust\nfn f() {}\n```\n";
        let blocks = parse(content);
        let Block::CodeFence(cf) = &blocks[0] else {
            panic!("expected code fence");
        };
        assert_eq!(cf.language, "rust");
        assert_eq!(text_of(content, cf.fence_open.unwrap()), "```rust");
        assert_eq!(text_of(content, cf.fence_close.unwrap()), "```");
        // `content` runs up to the start of the closing fence LINE, so it
        // keeps the content line's own trailing `\n` verbatim (§1.4.5); the
        // emitter (`emit.rs`) is what clips per-line at render time.
        assert_eq!(text_of(content, cf.content), "fn f() {}\n");
    }

    #[test]
    fn blockquote_marker_per_line() {
        let content = "> line one\n> line two\n";
        let blocks = parse(content);
        let Block::Blockquote(bq) = &blocks[0] else {
            panic!("expected blockquote");
        };
        assert_eq!(bq.markers.len(), 2);
        assert_eq!(text_of(content, bq.markers[0].marker), "> ");
        assert_eq!(bq.markers[0].line, 0);
        assert_eq!(bq.markers[1].line, 1);
    }

    #[test]
    fn tasklist_marker_and_task_range() {
        let content = "- [x] task\n";
        let blocks = parse(content);
        let Block::List(list) = &blocks[0] else {
            panic!("expected list");
        };
        assert_eq!(list.items.len(), 1);
        let item = &list.items[0];
        assert_eq!(text_of(content, item.marker), "- [x] ");
        assert_eq!(text_of(content, item.task.unwrap()), "[x]");
    }

    #[test]
    fn wikilink_target_and_label() {
        let content = "[[wiki|label]]\n";
        let blocks = parse(content);
        let Block::Paragraph(p) = &blocks[0] else {
            panic!("expected paragraph");
        };
        let Inline::WikiLink(w) = &p.inlines[0] else {
            panic!("expected wikilink");
        };
        assert_eq!(w.target, "wiki");
        assert_eq!(text_of(content, w.label), "label");
    }

    #[test]
    fn table_and_html_block_become_verbatim() {
        let content = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse(content);
        assert!(matches!(blocks[0], Block::Verbatim(_)));
    }

    #[test]
    fn frontmatter_is_pinned_revealed() {
        let content = "---\ntitle: x\n---\nbody\n";
        let blocks = parse(content);
        let Block::Frontmatter(fm) = &blocks[0] else {
            panic!("expected frontmatter, got {:?}", blocks[0]);
        };
        assert_eq!(fm.sm.state(), RevealState::Revealed);
    }

    #[test]
    fn inline_image_is_plain_text_run() {
        let content = "![alt](img.png)\n";
        let blocks = parse(content);
        let Block::Paragraph(p) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(matches!(p.inlines[0], Inline::Text(_)));
        let Inline::Text(t) = &p.inlines[0] else {
            unreachable!()
        };
        assert_eq!(text_of(content, t.range), "![alt](img.png)");
    }
}
