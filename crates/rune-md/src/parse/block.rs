//! AST -> `Block` construction: the top-level dispatch (`build_block`) and
//! the block kinds that recurse into further blocks (`BlockQuote`, `List`).

use super::blockquote::blockquote_markers;
use super::{ScanHint, line_end_at, node_range};
use crate::element::block::{
    Block, BlockquoteM, CodeFenceM, FrontmatterM, HeadingM, HrM, ListItemM, ListM, ParagraphM,
    VerbatimKind, VerbatimM,
};
use comrak::nodes::{AstNode, ListType, NodeValue};
use rune_syntax::element::{ByteRange, RevealSm, RevealState};

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

            Some(Block::Heading(HeadingM {
                sm: RevealSm::new(RevealState::Rendered),
                level,
                line,
                range,
                marker,
                inlines,
                content_lines,
            }))
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
                let content_lines = super::per_line_content(content, starts, range, hint);
                return Some(Block::Verbatim(VerbatimM {
                    sm: RevealSm::new(RevealState::Revealed),
                    range,
                    // Verbatim like every other passthrough, but tagged as
                    // code so the code-region collection can find it: an
                    // indented code block is code, an unrecognized node is
                    // not, and `Unknown` alone could not tell them apart.
                    kind: VerbatimKind::IndentedCode,
                    content_lines,
                }));
            }
            // `first_line`/`last_line` stay BUFFER-derived — they're
            // stored on `CodeFenceM` and read ONLY by the cursor-reveal
            // decide policy (`cursors.any_in_lines`, comparing against the
            // cursor's own buffer row).
            let first_line = line;
            let last_line = super::line_at(starts, range.end.saturating_sub(1).max(range.start));

            // A fence's OWN internal physical-line structure — where its
            // opening fence line ends, where each content line starts,
            // where the closing fence line begins — is derived from
            // `starts`, the same index `fence_open`/`fence_close`/
            // `content_lines` and every consumer of `line_at` share, so
            // this recovers exactly the fence's own recognized lines
            // (`comrak_first_line`..=`comrak_last_line`), mirroring the
            // setext-heading fix above.
            let comrak_first_line = super::line_at(starts, range.start);
            let comrak_last_line =
                super::line_at(starts, range.end.saturating_sub(1).max(range.start));

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
            let comrak_first_line_end = line_end_at(content.len(), starts, comrak_first_line);
            let fence_open =
                Some(ByteRange::new(range.start, comrak_first_line_end).clamp(content.len()));

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
                let ls = hint.start_for_line(starts, comrak_last_line);
                let le = line_end_at(content.len(), starts, comrak_last_line);
                let close = ByteRange::new(ls, le).clamp(content.len());
                (Some(close), (comrak_first_line + 1)..comrak_last_line)
            } else {
                (None, (comrak_first_line + 1)..(comrak_last_line + 1))
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
        BlockKind::ThematicBreak => {
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
            Some(Block::ThematicBreak(HrM {
                sm: RevealSm::new(RevealState::Rendered),
                line,
                range,
            }))
        }
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
        BlockKind::FrontMatter => Some(Block::Frontmatter(FrontmatterM {
            sm: RevealSm::new(RevealState::Revealed),
            range,
        })),
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
                // passthrough every other unmodeled construct gets (§0),
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
        let comrak_line = super::line_at(starts, range.start);
        let marker_end = item_node
            .first_child()
            .map(|c| node_range(content, starts, c).start)
            .unwrap_or(range.end)
            .max(range.start)
            .min(range.end)
            .min(line_end_at(content.len(), starts, comrak_line));
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

/// True if `range`'s own last line — as comrak (via our conversion)
/// reports it — is genuinely a closing `"---"` line: the sanity check
/// `parse::frontmatter_extension_is_safe` uses to decide whether a
/// `FrontMatter` node's reported range can be trusted at all (see that
/// function's docs for the comrak-internal desync it exists to detect).
pub(super) fn is_valid_frontmatter_close(content: &str, range: ByteRange) -> bool {
    let Some(text) = content.get(range.start..range.end) else {
        return false;
    };
    let trimmed = text.strip_suffix('\n').unwrap_or(text);
    trimmed.rsplit('\n').next() == Some("---")
}
