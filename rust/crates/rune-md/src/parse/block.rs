//! AST -> `Block` construction: the top-level dispatch (`build_block`) and
//! the block kinds that recurse into further blocks (`BlockQuote`, `List`).

use super::{ScanHint, line_end_at, node_range};
use crate::element::block::{
    Block, BlockquoteM, BlockquoteMarkerM, CodeFenceM, FrontmatterM, HeadingM, HrM, ListItemM,
    ListM, ParagraphM, VerbatimKind, VerbatimM,
};
use crate::element::{ByteRange, RevealSm, RevealState};
use comrak::nodes::{AstNode, ListType, NodeValue};

/// A block-node dispatch key extracted from `NodeValue` up front, so the
/// borrow on `node.data` can be dropped before any recursive call re-borrows
/// the same arena (comrak's `Ast` is a `RefCell`; a live borrow across a
/// recursive `build_blocks`/`build_inlines` call would panic at runtime).
enum BlockKind {
    Document,
    Paragraph,
    Heading(u8),
    BlockQuote,
    CodeBlock {
        fenced: bool,
        info: String,
        closed: bool,
    },
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
                closed: cb.closed,
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

pub(super) fn build_blocks<'a>(
    content: &str,
    starts: &[usize],
    parent: &'a AstNode<'a>,
    hint: &ScanHint,
) -> Vec<Block> {
    let mut out = Vec::new();
    for child in parent.children() {
        if let Some(b) = build_block(content, starts, child, hint) {
            out.push(b);
        }
    }
    out
}

fn build_block<'a>(
    content: &str,
    starts: &[usize],
    node: &'a AstNode<'a>,
    hint: &ScanHint,
) -> Option<Block> {
    let range = node_range(content, starts, node);
    let sp = node.data.borrow().sourcepos;
    let line = sp.start.line.saturating_sub(1);

    let kind = { node.data.borrow().value.clone_kind_tag() };
    match kind {
        BlockKind::Paragraph => {
            let inlines = super::inline::build_inlines(content, starts, node, hint);
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
            let inlines = super::inline::build_inlines(content, starts, node, hint);
            Some(Block::Heading(HeadingM {
                sm: RevealSm::new(RevealState::Rendered),
                level,
                line,
                range,
                marker,
                inlines,
            }))
        }
        BlockKind::BlockQuote => {
            let markers = blockquote_markers(content, starts, range, hint);
            let marker_ends = markers.iter().map(|m| (m.line, m.marker.end)).collect();
            let child_hint = ScanHint::Nested {
                marker_ends,
                parent: hint,
            };
            let children = build_blocks(content, starts, node, &child_hint);
            Some(Block::Blockquote(BlockquoteM {
                range,
                markers,
                children,
            }))
        }
        BlockKind::CodeBlock {
            fenced,
            info,
            closed,
        } => {
            if !fenced {
                return Some(Block::Verbatim(VerbatimM {
                    sm: RevealSm::new(RevealState::Revealed),
                    range,
                    kind: VerbatimKind::Unknown,
                }));
            }
            let first_line = line;
            let last_line = sp.end.line.saturating_sub(1);

            // CONTAINER-PREFIX fix: `fence_open`'s start is `range.start`,
            // NOT `line_start_at`/`hint` — a node's own sourcepos already
            // bakes in EVERY ancestor's line-0 prefix (a blockquote's
            // `"> "` AND a list item's `"- "`/`"1. "`, whichever applies),
            // so `range.start` is already exactly where this fence's own
            // first line begins. `hint` only tracks REPEATING blockquote
            // markers across continuation lines — it has no entry for a
            // list item's non-repeating marker, so using it here (instead
            // of `range.start`) would silently fall back to the physical
            // line start and re-claim bytes the list item's own marker
            // already hid (`"- ```rust"` -> fence_open `[0, 9)` colliding
            // with the item's own marker `[0, 2)`).
            let first_line_end = line_end_at(content.len(), starts, first_line);
            let fence_open = Some(ByteRange::new(range.start, first_line_end).clamp(content.len()));

            // BLOCKER 3 fix (prior round): `last_line > first_line` alone is
            // NOT "a closing fence exists" — every fence is unterminated
            // while being typed (open fence + content, no closing ``` yet),
            // and that shape also has `last_line > first_line`. comrak
            // already tells us whether a real closing fence was matched
            // (`NodeCodeBlock::closed`); trust it instead of inferring from
            // line span. Unclosed -> no `fence_close`, and every byte after
            // the opening fence line through the end of the block is live
            // content (never silently concealed as if it were a fence).
            //
            // CONTAINER-PREFIX fix (this round): `fence_close`'s start and
            // EVERY content line's start use `hint.start_for_line` — unlike
            // `fence_open`'s own first line, these are CONTINUATION lines of
            // the fence, which is exactly what `hint` exists to handle
            // (skip a repeating blockquote `"> "` on that physical line; a
            // no-op physical-line-start for a list item, which has nothing
            // to skip past line 0). Each content line gets its OWN range —
            // never one contiguous span across lines — because a single
            // range can't exclude an interior container prefix (the
            // overlapping-hidden-range bug this fix exists to close).
            let (fence_close, content_line_range) = if closed {
                let ls = hint.start_for_line(starts, last_line);
                let le = line_end_at(content.len(), starts, last_line);
                let close = ByteRange::new(ls, le).clamp(content.len());
                (Some(close), (first_line + 1)..last_line)
            } else {
                (None, (first_line + 1)..(last_line + 1))
            };

            let content_lines: Vec<ByteRange> = content_line_range
                .map(|l| {
                    let s = hint.start_for_line(starts, l);
                    let e = line_end_at(content.len(), starts, l);
                    ByteRange::new(s, e).clamp(content.len())
                })
                .collect();

            Some(Block::CodeFence(CodeFenceM {
                sm: RevealSm::new(RevealState::Rendered),
                range,
                first_line,
                last_line,
                language: info,
                fence_open,
                fence_close,
                content_lines,
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
            let items = build_list_items(content, starts, node, hint);
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

fn build_list_items<'a>(
    content: &str,
    starts: &[usize],
    list_node: &'a AstNode<'a>,
    hint: &ScanHint,
) -> Vec<ListItemM> {
    let mut items = Vec::new();
    for item_node in list_node.children() {
        let range = node_range(content, starts, item_node);
        let sp = item_node.data.borrow().sourcepos;
        let line = sp.start.line.saturating_sub(1);

        let task = match &item_node.data.borrow().value {
            NodeValue::TaskItem(t) => {
                let sym = super::sourcepos_to_range(starts, t.symbol_sourcepos);
                let bracket_start = sym.start.saturating_sub(1);
                let bracket_end = sym.end.saturating_add(1).min(content.len());
                Some(ByteRange::new(bracket_start, bracket_end).clamp(content.len()))
            }
            _ => None,
        };

        // BLOCKER fix: an item's marker can never span lines. For an EMPTY
        // item (`"- "`, bare `"-"`, `"1."`, ...) the first child is whatever
        // block starts the item's CONTENT, which for a lazily-indented
        // continuation (e.g. a nested blockquote under `"- \n  > q"`) sits
        // on the NEXT physical line — so `first_child.start` alone let the
        // marker run past the item's own line-0 end and swallow line 1's
        // leading indent, bytes the continuation's own scan (e.g.
        // `blockquote_markers`) claims independently. Clamping to this
        // line's own end makes "a marker never crosses a line boundary" a
        // structural guarantee instead of something every call site has to
        // get right.
        let marker_end = item_node
            .first_child()
            .map(|c| node_range(content, starts, c).start)
            .unwrap_or(range.end)
            .max(range.start)
            .min(range.end)
            .min(line_end_at(content.len(), starts, line));
        let marker = ByteRange::new(range.start, marker_end);
        let children = build_blocks(content, starts, item_node, hint);

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
///
/// MAJOR 4 fix: for a NESTED blockquote (`"> > nested"`), comrak reports
/// each depth's own `BlockQuote` node sourcepos starting right AFTER the
/// outer level's `"> "` prefix on line 0 — verified empirically: for
/// `"> > nested quote\n"` the outer node's sourcepos is `1:1-1:16` and the
/// inner's is `1:3-1:16` (column 3 = byte offset 2, right past the outer
/// `"> "`). But that per-line signal only exists for line 0 — a multi-line
/// nested blockquote (`"> > nested\n> > nested"`) needs the SAME
/// depth-aware scan-start on every continuation line too, which comrak's
/// sourcepos doesn't give us (only the node's overall start/end). `hint`
/// (built by the caller from the immediately enclosing depth's own
/// just-computed markers) supplies it uniformly for every line, line 0
/// included — see `ScanHint`'s docs.
fn blockquote_markers(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    hint: &ScanHint,
) -> Vec<BlockquoteMarkerM> {
    let first_line = super::line_at(starts, range.start);
    let last_line = super::line_at(starts, range.end.saturating_sub(1).max(range.start));
    let mut markers = Vec::new();
    for line in first_line..=last_line {
        let line_end = line_end_at(content.len(), starts, line);
        let scan_start = hint.start_for_line(starts, line);
        let Some(line_text) = content.get(scan_start..line_end.max(scan_start)) else {
            continue;
        };
        // RESIDUAL PRODUCER fix (verification round 3, ">]\n\t>"): only
        // SPACES (never a tab), capped at 3, count as marker-prefix
        // indentation — CommonMark's own indentation rule for a repeated
        // block-container marker. `str::trim_start()` strips ANY
        // whitespace including tabs, so it used to also recognize a
        // tab-indented line ("\t>") as a repeated ">" marker; comrak
        // itself does NOT (a leading tab represents 4 columns, past the
        // 3-space budget, so it treats the line as lazy-continuation
        // paragraph TEXT instead). That mismatch meant this scan and the
        // paragraph's own Text node both claimed the same "> " bytes — a
        // producer-bug double-claim `push_span_split_by_line` now catches
        // via `assert_invariant`. Capping at 3 spaces (not just excluding
        // tabs outright) keeps a legitimately 1-3-space-indented
        // continuation ("   > cont") recognized, matching comrak exactly.
        let ws_len = line_text.bytes().take_while(|&b| b == b' ').count().min(3);
        let Some(trimmed) = line_text.get(ws_len..) else {
            continue;
        };
        if let Some(rest) = trimmed.strip_prefix('>') {
            let mut marker_len = ws_len + 1;
            if rest.starts_with(' ') {
                marker_len += 1;
            }
            let marker_end = scan_start.saturating_add(marker_len).min(content.len());
            markers.push(BlockquoteMarkerM {
                sm: RevealSm::new(RevealState::Rendered),
                line,
                marker: ByteRange::new(scan_start, marker_end),
            });
        }
    }
    markers
}
