use super::blockquote::blockquote_markers;
use super::{ScanHint, last_line_of, line_end_at, node_range};
use crate::element::block::{
    Block, BlockquoteM, CodeFenceM, HeadingM, HrM, ListItemM, ListM, ParagraphM, VerbatimKind,
    VerbatimM,
};
use crate::element::inline::Inline;
use comrak::nodes::{AstNode, ListType, NodeValue};
use rune_syntax::element::{ByteRange, RevealSm, RevealState};

// comrak's `Ast` is a `RefCell`; extracting the node kind up front lets the
// borrow on `node.data` drop before any recursive call re-borrows the same
// arena, which would otherwise panic at runtime.
enum BlockKind {
    Document,
    Paragraph,
    Heading {
        level: u8,
        setext: bool,
    },
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
            NodeValue::Heading(h) => BlockKind::Heading {
                level: h.level,
                setext: h.setext,
            },
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

#[derive(Clone, Copy)]
struct BlockCtx<'c, 'p> {
    content: &'c str,
    starts: &'c [usize],
    hint: &'c ScanHint<'p>,
    line: usize,
}

// comrak caps a LIST's own nesting at 100 (its `Options::parse::relaxed_*`
// tunables don't reach this), but a BLOCKQUOTE has no such cap: its tree
// depth is exactly the source's own run of leading `>` markers, unbounded.
// `build_block` recurses once per container level for either kind, so one
// shared counter — checked before EITHER arm recurses — bounds the real
// call-stack depth regardless of which kind (or which mix of the two)
// supplies the nesting, mirroring comrak's own list cap rather than
// inventing a different ceiling.
const MAX_CONTAINER_DEPTH: usize = 100;

pub(super) fn build_blocks<'a>(
    content: &str,
    starts: &[usize],
    parent: &'a AstNode<'a>,
    hint: &ScanHint,
    depth: usize,
) -> Vec<Block> {
    let mut out = Vec::new();
    for child in parent.children() {
        if let Some(b) = build_block(content, starts, child, hint, depth) {
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
    depth: usize,
) -> Option<Block> {
    let range = node_range(content, starts, node);
    // comrak's line numbers are 1-based and count `\r\n` pairs differently
    // than a plain `\n`-line index, so `line` is derived from the byte
    // range via `starts` instead of trusting comrak's own line number.
    let line = super::line_at(starts, range.start);

    let kind = { node.data.borrow().value.clone_kind_tag() };
    let ctx = BlockCtx {
        content,
        starts,
        hint,
        line,
    };
    match kind {
        BlockKind::Paragraph => {
            let inlines = super::inline::build_inlines(content, starts, node, hint);
            Some(Block::Paragraph(ParagraphM { range, inlines }))
        }
        BlockKind::Heading { level, setext } => {
            Some(build_heading(&ctx, node, range, level, setext))
        }
        BlockKind::BlockQuote if depth >= MAX_CONTAINER_DEPTH => Some(build_verbatim(
            content,
            starts,
            range,
            hint,
            VerbatimKind::Unknown,
        )),
        BlockKind::BlockQuote => {
            let markers = blockquote_markers(content, starts, range, hint);
            let marker_ends = markers
                .iter()
                .map(|m| (super::line_at(starts, m.marker.start), m.marker.end))
                .collect();
            let child_hint = ScanHint::Nested {
                marker_ends,
                conceals_own_prefix: true,
                parent: hint,
            };
            let children = build_blocks(content, starts, node, &child_hint, depth + 1);
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
        } => Some(build_code_block(&ctx, range, fenced, info, closed)),
        BlockKind::ThematicBreak => Some(build_thematic_break(content, starts, range, line)),
        BlockKind::List if depth >= MAX_CONTAINER_DEPTH => Some(build_verbatim(
            content,
            starts,
            range,
            hint,
            VerbatimKind::Unknown,
        )),
        BlockKind::List => {
            let ordered = matches!(
                node.data.borrow().value,
                NodeValue::List(ref l) if matches!(l.list_type, ListType::Ordered)
            );
            let items = build_list_items(content, starts, node, hint, depth + 1);
            Some(Block::List(ListM { ordered, items }))
        }
        // parse()'s `frontmatter_extension_is_safe` pre-check already ruled
        // out the one shape whose comrak-reported range can't be trusted,
        // so `range` here is always genuine.
        BlockKind::FrontMatter => Some(Block::Frontmatter(super::frontmatter::build(
            content, starts, range, hint,
        ))),
        BlockKind::Table => {
            super::table::build_table(content, starts, node, hint, range).or_else(|| {
                Some(build_verbatim(
                    content,
                    starts,
                    range,
                    hint,
                    VerbatimKind::Table,
                ))
            })
        }
        BlockKind::HtmlBlock => Some(build_verbatim(
            content,
            starts,
            range,
            hint,
            VerbatimKind::Html,
        )),
        BlockKind::Document => None,
        BlockKind::Other => Some(build_verbatim(
            content,
            starts,
            range,
            hint,
            VerbatimKind::Unknown,
        )),
    }
}

/// A block that degrades to raw passthrough text: unrecognized syntax
/// (`BlockKind::Other`/`HtmlBlock`), a malformed table falling back off
/// `build_table`, and a blockquote/list nested past `MAX_CONTAINER_DEPTH`
/// (never structured further — see that constant's docs) all share this
/// exact shape.
fn build_verbatim(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    hint: &ScanHint,
    kind: VerbatimKind,
) -> Block {
    Block::Verbatim(VerbatimM {
        sm: RevealSm::new(RevealState::Revealed),
        range,
        kind,
        content_lines: super::per_line_content(content, starts, range, hint),
    })
}

fn build_heading<'a>(
    ctx: &BlockCtx,
    node: &'a AstNode<'a>,
    range: ByteRange,
    level: u8,
    setext: bool,
) -> Block {
    let BlockCtx {
        content,
        starts,
        hint,
        line,
    } = *ctx;
    let marker_end = node
        .first_child()
        .map_or(range.end, |c| node_range(content, starts, c).start)
        .max(range.start)
        .min(range.end);
    let marker = ByteRange::new(range.start, marker_end);
    let inlines = super::inline::build_inlines(content, starts, node, hint);

    // comrak's `range` for a setext heading spans both its text line and
    // its "==="/"---" underline; an ATX heading's range is always
    // single-line.
    let content_lines = super::per_line_content(content, starts, range, hint);

    let underline = underline_of_setext_heading(setext, &content_lines, &inlines).map(|u| {
        let underline_line = super::line_at(starts, u.start);
        let baseline = hint.concealment_baseline(starts, underline_line);
        ByteRange::new(baseline, u.end)
    });
    let last_line = last_line_of(starts, range);

    Block::Heading(HeadingM {
        sm: RevealSm::new(RevealState::Rendered),
        level,
        line,
        last_line,
        range,
        setext,
        marker,
        underline,
        inlines,
        content_lines,
    })
}

fn build_code_block(
    ctx: &BlockCtx,
    range: ByteRange,
    fenced: bool,
    info: String,
    closed: bool,
) -> Block {
    let BlockCtx {
        content,
        starts,
        hint,
        line,
    } = *ctx;
    if !fenced {
        // comrak reports an indented code block's `range.start` past its
        // own leading indentation; `width` recovers that amount so every
        // continuation line strips the same fixed width.
        let baseline = hint.start_for_line(starts, line);
        let width = range.start.saturating_sub(baseline);
        let marker_ends = super::indent::fixed_indent_ends(content, starts, range, width, hint);
        let local_hint = ScanHint::Nested {
            marker_ends,
            conceals_own_prefix: false,
            parent: hint,
        };
        let content_lines = super::per_line_content(content, starts, range, &local_hint);
        return Block::Verbatim(VerbatimM {
            sm: RevealSm::new(RevealState::Revealed),
            range,
            kind: VerbatimKind::IndentedCode,
            content_lines,
        });
    }
    let first_line = line;
    let last_line = last_line_of(starts, range);

    // An open fence with no closing ``` also has `last_line > first_line`,
    // so use comrak's `NodeCodeBlock::closed` rather than inferring "has a
    // closing fence" from line span.
    let lines = super::delimited::split(content, starts, range, hint, closed);

    Block::CodeFence(CodeFenceM {
        sm: RevealSm::new(RevealState::Rendered),
        range,
        first_line,
        last_line,
        language: info,
        fence_open: lines.open,
        fence_close: lines.close,
        content_lines: lines.content_lines,
    })
}

fn build_thematic_break(content: &str, starts: &[usize], range: ByteRange, line: usize) -> Block {
    // comrak's range for a thematic break immediately followed by an empty
    // blockquote continuation line (e.g. `"> ---\n>"`) extends through
    // that next line's own "> " marker instead of stopping at the break;
    // clamp to this line's own end.
    let comrak_line = super::line_at(starts, range.start);
    let clamped_end = line_end_at(content.len(), starts, comrak_line)
        .min(range.end)
        .max(range.start);
    let range = ByteRange::new(range.start, clamped_end).clamp(content.len());
    Block::ThematicBreak(HrM {
        sm: RevealSm::new(RevealState::Rendered),
        line,
        range,
    })
}

fn build_list_items<'a>(
    content: &str,
    starts: &[usize],
    list_node: &'a AstNode<'a>,
    hint: &ScanHint,
    depth: usize,
) -> Vec<ListItemM> {
    let mut items = Vec::new();
    for item_node in list_node.children() {
        let range = node_range(content, starts, item_node);
        let line = super::line_at(starts, range.start);

        let task = match &item_node.data.borrow().value {
            NodeValue::TaskItem(t) => {
                let sym = super::sourcepos_to_range(content, starts, t.symbol_sourcepos);
                let bracket_start = sym.start.saturating_sub(1);
                let bracket_end = sym.end.saturating_add(1).min(content.len());
                Some(ByteRange::new(bracket_start, bracket_end).clamp(content.len()))
            }
            _ => None,
        };

        let comrak_line = super::line_at(starts, range.start);
        let marker_end = item_node
            .first_child()
            .map_or(range.end, |c| node_range(content, starts, c).start)
            .max(range.start)
            .min(range.end)
            .min(line_end_at(content.len(), starts, comrak_line));
        let marker = ByteRange::new(range.start, marker_end);

        let width = marker_end.saturating_sub(range.start);
        let marker_ends = super::indent::fixed_indent_ends(content, starts, range, width, hint);
        let child_hint = ScanHint::Nested {
            marker_ends,
            conceals_own_prefix: false,
            parent: hint,
        };
        let children = build_blocks(content, starts, item_node, &child_hint, depth);

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

fn underline_of_setext_heading(
    setext: bool,
    content_lines: &[ByteRange],
    inlines: &[Inline],
) -> Option<ByteRange> {
    if !setext {
        return None;
    }
    // comrak can leave a residual inline `Text` node covering the same
    // bytes as a setext underline it already recognized (e.g. an unmatched
    // emphasis delimiter trailing into the underline line:
    // `"x\n*a\nb\n---\n"`).
    content_lines.last().copied().filter(|underline| {
        !inlines
            .iter()
            .any(|i| ranges_overlap(i.range(), *underline))
    })
}

fn ranges_overlap(a: ByteRange, b: ByteRange) -> bool {
    a.start < b.end && b.start < a.end
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

    /// `clone_kind_tag`'s `NodeValue::Document` arm is never reached through
    /// `parse()`: comrak's own AST has exactly one `Document` node, the
    /// root `build_blocks` iterates the CHILDREN of, never a node
    /// `build_block` itself is called on — so this pins the trait method's
    /// own contract directly, the same way `catalogue`'s `heading_name`
    /// test above pins a branch no real `parse()` input can reach.
    #[test]
    fn document_classifies_as_its_own_kind_not_the_generic_fallback() {
        assert!(matches!(
            NodeValue::Document.clone_kind_tag(),
            BlockKind::Document
        ));
    }

    /// An HTML block degrades to `Verbatim` the same way any unrecognized
    /// syntax does, but must keep ITS OWN `VerbatimKind::Html` tag rather
    /// than falling through to `clone_kind_tag`'s generic
    /// `_ => BlockKind::Other` arm (which produces `VerbatimKind::Unknown`
    /// instead) — a real, reachable distinction the existing
    /// `table_and_html_block_become_verbatim` test (which only checks
    /// `matches!(_, Block::Verbatim(_))`) does not pin.
    #[test]
    fn html_block_keeps_its_own_verbatim_kind() {
        let content = "<div>\nraw\n</div>\n";
        let blocks = crate::parse::parse(content);
        let Block::Verbatim(v) = &blocks[0] else {
            panic!("expected a verbatim block, got {:?}", blocks[0]);
        };
        assert_eq!(v.kind, VerbatimKind::Html);
    }

    /// One new blockquote level per line (never more than one NEW container
    /// per line) keeps every level comrak actually opens attributed to
    /// THIS crate's own `MAX_CONTAINER_DEPTH` cap alone — comrak's OWN
    /// `MAX_LIST_DEPTH` (also 100) gates only how many containers a SINGLE
    /// line may newly open at once, so building the nesting incrementally
    /// like this is what lets a depth of 100+ actually form at all.
    fn nested_blockquotes(levels: usize) -> String {
        let mut content = String::new();
        for k in 1..=levels {
            content.push_str(&">".repeat(k));
            content.push_str(" x\n");
        }
        content
    }

    fn deepest_kind(content: &str) -> Block {
        let blocks = crate::parse::parse(content);
        let mut b = blocks.into_iter().next().expect("at least one block");
        loop {
            match b {
                Block::Blockquote(mut bq) => {
                    b = bq.children.pop().expect("blockquote must have a child");
                }
                other => return other,
            }
        }
    }

    /// A list met at `depth == 99` (inside 99 blockquote levels) is still a
    /// real `List`; at `depth == 100` the `BlockKind::List` cap guard is the
    /// only thing degrading it to `Verbatim` — a guard hard-wired to `false`
    /// would keep nesting real lists forever.
    #[test]
    fn a_list_at_the_container_depth_cap_degrades_to_verbatim() {
        let mut under = nested_blockquotes(99);
        under.push_str(&">".repeat(99));
        under.push_str(" - x\n");
        assert!(matches!(deepest_kind(&under), Block::List(_)));

        let mut over = nested_blockquotes(100);
        over.push_str(&">".repeat(100));
        over.push_str(" - x\n");
        assert!(matches!(deepest_kind(&over), Block::Verbatim(_)));
    }

    /// Exactly 100 real blockquote levels (the 100th built at `depth == 99`,
    /// still under the cap) still nest normally; a 101st (built at
    /// `depth == 100`) is the first to degrade to `Verbatim` — the precise
    /// boundary `depth >= MAX_CONTAINER_DEPTH` draws. A guard hard-wired to
    /// `false` would never degrade the 101st level either.
    #[test]
    fn the_hundred_and_first_nested_blockquote_degrades_to_verbatim() {
        assert!(matches!(
            deepest_kind(&nested_blockquotes(100)),
            Block::Paragraph(_)
        ));
        assert!(matches!(
            deepest_kind(&nested_blockquotes(101)),
            Block::Verbatim(_)
        ));
    }

    /// A list nested 99 blockquote levels deep is still real (`depth == 99`
    /// going in, under the cap); building its OWN item's child depth as
    /// `depth + 1` rather than `depth` is what then correctly pushes a
    /// further-nested container over the cap at exactly `depth == 100` —
    /// pinned by checking that container degrades to `Verbatim`, which a
    /// `depth * 1` (no-op) miscount would never do.
    #[test]
    fn a_list_items_own_child_depth_is_one_more_than_the_lists_own() {
        let mut content = nested_blockquotes(98);
        content.push_str(&">".repeat(99));
        content.push_str(" - > x\n");
        let blocks = crate::parse::parse(&content);
        let mut b = blocks.into_iter().next().expect("at least one block");
        let list = loop {
            match b {
                Block::Blockquote(mut bq) => {
                    b = bq.children.pop().expect("blockquote must have a child");
                }
                Block::List(list) => break list,
                other => panic!("expected to reach a list, got {other:?}"),
            }
        };
        let item = list.items.into_iter().next().expect("one list item");
        assert_eq!(item.children.len(), 1);
        assert!(
            matches!(item.children[0], Block::Verbatim(_)),
            "expected the item's own nested blockquote to degrade to Verbatim at depth 100, got {:?}",
            item.children[0]
        );
    }

    /// `ranges_overlap`'s only real caller (`underline_of_setext_heading`)
    /// is a defensive guard for a comrak inline/block desync this crate's
    /// own `parse()` cannot currently reproduce (see that function's docs
    /// and `catalogue`'s own `setext_heading_name_survives_a_degraded_
    /// underline` test) — pinned directly against its own half-open-range
    /// contract instead.
    #[test]
    fn ranges_overlap_treats_touching_ranges_as_not_overlapping() {
        assert!(ranges_overlap(ByteRange::new(0, 10), ByteRange::new(5, 15)));
        assert!(!ranges_overlap(ByteRange::new(0, 5), ByteRange::new(5, 10)));
        assert!(!ranges_overlap(ByteRange::new(5, 10), ByteRange::new(0, 5)));
        assert!(!ranges_overlap(
            ByteRange::new(0, 5),
            ByteRange::new(10, 15)
        ));
    }
}
