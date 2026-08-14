//! AST -> `Block` construction: the top-level dispatch (`build_block`) and
//! the block kinds that recurse into further blocks (`BlockQuote`, `List`).

use super::blockquote::blockquote_markers;
use super::{ScanHint, last_line_of, line_end_at, node_range};
use crate::element::block::{
    Block, BlockquoteM, CodeFenceM, HeadingM, HrM, ListItemM, ListM, ParagraphM, VerbatimKind,
    VerbatimM,
};
use crate::element::inline::Inline;
use comrak::nodes::{AstNode, ListType, NodeValue};
use rune_syntax::element::{ByteRange, RevealSm, RevealState};

/// A block-node dispatch key extracted from `NodeValue` up front, so the
/// borrow on `node.data` can be dropped before any recursive call re-borrows
/// the same arena (comrak's `Ast` is a `RefCell`; a live borrow across a
/// recursive `build_blocks`/`build_inlines` call would panic at runtime).
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

/// The parse-wide facts `build_heading`/`build_code_block` both need,
/// bundled so they stay under clippy's too-many-arguments lint without an
/// `#[allow]` (repo rule: none outside test code) — the same
/// "bundle instead of allow" shape `rune-db`'s `LoadContext` already uses.
#[derive(Clone, Copy)]
struct BlockCtx<'c, 'p> {
    content: &'c str,
    starts: &'c [usize],
    hint: &'c ScanHint<'p>,
    line: usize,
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
    // BUFFER line, not comrak's own `sp.start.line`: every consumer of
    // `line` — the cursor-reveal decide policy (`any_on_line`), and every
    // `hint`/`line_end_at` loop bound below — reasons about the EDITOR's
    // `\n`-only line concept. Deriving `line` from the already-correct
    // absolute byte range (via `starts`) instead of trusting comrak's raw
    // line number keeps it meaningful even where comrak's own numbering
    // (1-based, and off-by-however-many `\r\n` pairs it counted as one
    // terminator) diverges from a plain line index.
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
        BlockKind::BlockQuote => {
            let markers = blockquote_markers(content, starts, range, hint);
            // Keyed by each marker's own line as `starts` counts it, NOT
            // `m.line` (the SAME value here, but `BlockquoteMarkerM`
            // stores `.line` for the cursor-reveal decide policy — a
            // different purpose, kept as its own field rather than reused
            // as a map key by coincidence).
            let marker_ends = markers
                .iter()
                .map(|m| (super::line_at(starts, m.marker.start), m.marker.end))
                .collect();
            let child_hint = ScanHint::Nested {
                marker_ends,
                conceals_own_prefix: true,
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
        } => Some(build_code_block(&ctx, range, fenced, info, closed)),
        BlockKind::ThematicBreak => Some(build_thematic_break(content, starts, range, line)),
        BlockKind::List => {
            let ordered = matches!(
                node.data.borrow().value,
                NodeValue::List(ref l) if matches!(l.list_type, ListType::Ordered)
            );
            let items = build_list_items(content, starts, node, hint);
            Some(Block::List(ListM { ordered, items }))
        }
        // By the time `build_block` ever sees a `FrontMatter` node,
        // `parse()`'s own pre-check (`frontmatter_extension_is_safe`) has
        // already ruled out the ONE shape whose `range` can't be trusted
        // (verification round 5 — see that function's docs) by re-
        // parsing the whole document with the extension disabled instead
        // — so `range` here is always genuine.
        BlockKind::FrontMatter => Some(Block::Frontmatter(super::frontmatter::build(
            content, starts, range, hint,
        ))),
        BlockKind::Table => {
            super::table::build_table(content, starts, node, hint, range).or_else(|| {
                // `build_table` returns `None` on anything unexpected: a
                // non-`Table` node reaching this arm; a table with no rows;
                // a body row and the derived delimiter line landing on the
                // same buffer line (a desync between the buffer's own line
                // index and comrak's, so the collision would otherwise
                // render one display row carrying two rows' worth of
                // cells); or the table's range starting at a mid-line
                // position not explained by the scan hint's container
                // prefix (comrak would then report every later row's cell
                // sourcepos shifted, rendering every cell missing its first
                // character). In every case, degrade to the same raw
                // passthrough every other unmodeled construct gets,
                // never panic or render the user's words wrongly.
                Some(Block::Verbatim(VerbatimM {
                    sm: RevealSm::new(RevealState::Revealed),
                    range,
                    kind: VerbatimKind::Table,
                    content_lines: super::per_line_content(content, starts, range, hint),
                }))
            })
        }
        BlockKind::HtmlBlock => Some(Block::Verbatim(VerbatimM {
            sm: RevealSm::new(RevealState::Revealed),
            range,
            kind: VerbatimKind::Html,
            content_lines: super::per_line_content(content, starts, range, hint),
        })),
        BlockKind::Document => None,
        BlockKind::Other => Some(Block::Verbatim(VerbatimM {
            sm: RevealSm::new(RevealState::Revealed),
            range,
            kind: VerbatimKind::Unknown,
            content_lines: super::per_line_content(content, starts, range, hint),
        })),
    }
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

    // MAJOR fix (verification round 4): a setext heading's own
    // `range` spans BOTH its text line and its "==="/"---"
    // underline — feeding that whole multi-line span straight
    // into the generic per-physical-line splitter (as the
    // Revealed emit path used to) re-claims a REPEATING container
    // prefix (a blockquote's "> ") on the underline's own
    // continuation line, on top of whatever the blockquote's own
    // marker scan already (and correctly) claims there — the
    // exact "fence-inside-container" class `CodeFenceM` was
    // already fixed for. An ATX heading is always single-line
    // (`comrak_last_line == comrak_first_line`), so this is a
    // no-op `vec![range]`; only a setext heading needs the
    // per-line, `hint`-aware breakdown — first line trusts
    // `range.start` (a block's own sourcepos-derived first-line
    // start is always reliable, the same assumption
    // `CodeFenceM::fence_open` relies on), every CONTINUATION
    // line uses `hint.start_for_line` to skip a repeating
    // container prefix comrak's sourcepos alone can't be trusted
    // to exclude.
    //
    // Built via `per_line_content`, which iterates and clamps by
    // `starts` — a setext heading's own two "lines" (text +
    // underline) are always exactly two entries in `content_lines`
    // regardless of how many lines this node itself spans. `line`
    // (buffer-derived, stored on `HeadingM` for the cursor-reveal
    // decide policy) stays a separate field for that one purpose.
    let content_lines = super::per_line_content(content, starts, range, hint);

    // A setext heading's underline is always its LAST content line
    // — its text may itself span several lines (`Foo\nBar\n---`),
    // so `content_lines[1]` would be wrong for that shape.
    //
    // Widened past its own content-only start to `hint`'s own
    // `concealment_baseline`: a depth without an independent
    // concealment claim of its own (a list item's fixed
    // continuation width, unlike a blockquote's per-line
    // `BlockquoteMarkerM`) has nothing else that will ever hide
    // its prefix on this row, so the underline's own hide must
    // reach back and cover it.
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
        // Comrak reports `range.start` past this block's own
        // leading indentation (see `sourcepos_to_range`'s docs) —
        // `width` recovers exactly how much that is, so every
        // CONTINUATION line strips the same fixed amount instead of
        // trusting its raw physical start.
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
            // Verbatim like every other passthrough, but tagged as
            // code so the code-region collection can find it: an
            // indented code block is code, an unrecognized node is
            // not, and `Unknown` alone could not tell them apart.
            kind: VerbatimKind::IndentedCode,
            content_lines,
        });
    }
    // `first_line`/`last_line` stay BUFFER-derived — they're
    // stored on `CodeFenceM` and read ONLY by the cursor-reveal
    // decide policy (`cursors.any_in_lines`, comparing against the
    // cursor's own buffer row).
    let first_line = line;
    let last_line = last_line_of(starts, range);

    // BLOCKER 3 fix (prior round): `last_line > first_line` alone is
    // NOT "a closing fence exists" — every fence is unterminated
    // while being typed (open fence + content, no closing ``` yet),
    // and that shape also has `last_line > first_line`. comrak
    // already tells us whether a real closing fence was matched
    // (`NodeCodeBlock::closed`); trust it instead of inferring from
    // line span. Unclosed -> no `fence_close`, and every byte after
    // the opening fence line through the end of the block is live
    // content (never silently concealed as if it were a fence).
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
    // CLASS B fix (verification round 5): a thematic break is
    // ALWAYS exactly one line by CommonMark's own grammar (three
    // or more matching "-"/"_"/"*" chars, nothing else) — but
    // comrak's reported sourcepos for one immediately followed by
    // an EMPTY blockquote continuation line ("> ---\n>") extends
    // THROUGH that next line's own "> " marker (verified
    // empirically: for "> ---\n>", `range` came out `[2,7)` =
    // "---\n>", not just "---" = `[2,5)`). The blockquote's own
    // marker scan independently (and correctly) claims that same
    // trailing "> " byte, so pushing the HR's un-clamped `range`
    // whole doubles it up — a hidden-side double-claim: the
    // marker is BOTH counted hidden (by the blockquote's own
    // scan) and swept into the HR's own hidden/visible range.
    // Clamping to the HR's own single line makes "a thematic
    // break's range never crosses a line boundary" a structural
    // guarantee, the same shape as `ListItemM`'s marker clamp.
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
) -> Vec<ListItemM> {
    let mut items = Vec::new();
    for item_node in list_node.children() {
        let range = node_range(content, starts, item_node);
        // BUFFER line — see `build_block`'s docs on why `line` is derived
        // from the already-correct byte range, not comrak's raw line
        // number (verification round 5 CLASS A).
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
        let comrak_line = super::line_at(starts, range.start);
        let marker_end = item_node
            .first_child()
            .map_or(range.end, |c| node_range(content, starts, c).start)
            .max(range.start)
            .min(range.end)
            .min(line_end_at(content.len(), starts, comrak_line));
        let marker = ByteRange::new(range.start, marker_end);

        // A list item's continuation indent is a fixed width established
        // once from its own marker, not a marker rescanned per line like a
        // blockquote's `"> "` — `fixed_indent_ends` derives it and threads
        // it to every child the same way `BlockQuote` threads its own
        // per-line markers.
        let width = marker_end.saturating_sub(range.start);
        let marker_ends = super::indent::fixed_indent_ends(content, starts, range, width, hint);
        let child_hint = ScanHint::Nested {
            marker_ends,
            conceals_own_prefix: false,
            parent: hint,
        };
        let children = build_blocks(content, starts, item_node, &child_hint);

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

/// A setext heading's underline row, or `None` if concealing it would hide
/// bytes `inlines` is already claiming as visible text. Comrak can leave a
/// residual inline `Text` node covering the same bytes as the underline it
/// just recognized (observed with an unmatched emphasis delimiter trailing
/// into the underline line, e.g. `"x\n*a\nb\n---\n"` — the same class of
/// comrak-internal desync `frontmatter_extension_is_safe` and the wikilink
/// `within_brackets` guard already work around elsewhere in this crate).
fn underline_of_setext_heading(
    setext: bool,
    content_lines: &[ByteRange],
    inlines: &[Inline],
) -> Option<ByteRange> {
    if !setext {
        return None;
    }
    content_lines.last().copied().filter(|underline| {
        !inlines
            .iter()
            .any(|i| ranges_overlap(i.range(), *underline))
    })
}

/// Half-open byte-range overlap check.
fn ranges_overlap(a: ByteRange, b: ByteRange) -> bool {
    a.start < b.end && b.start < a.end
}
