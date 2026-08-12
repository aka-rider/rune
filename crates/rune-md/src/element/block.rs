//! Block-level element machines (plan Context, "Block and inline machines").
//! Dispatch is enum-match, no `dyn`. Each machine with a marker
//! (`RevealSm`) implements the fixed decide -> transition -> propagate shape
//! from the plan; `Paragraph` has no markers of its own and simply forwards
//! `ctx` to its inlines unchanged.

use crate::element::inline::Inline;
use crate::element::table::TableM;
use rune_syntax::element::{ByteRange, InheritCtx, RevealSm, RevealState};

/// A block with no delimiters of its own — plain prose. Forwards `ctx`
/// straight through to its inline children (plan: "no markers -> no
/// RevealSm; forwards ctx to inlines").
#[derive(Clone, Debug)]
pub struct ParagraphM {
    pub range: ByteRange,
    pub inlines: Vec<Inline>,
}

impl ParagraphM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let mut dirty = false;
        for c in &mut self.inlines {
            dirty |= c.sync(ctx);
        }
        dirty
    }
}

/// ATX heading (`## text`) or setext heading (`text\n===`/`text\n---`).
/// Decide policy: `cursors.any_in_lines(line, last_line)` (the reveal-policy
/// table) — the heading's own line span, text AND underline together, so a
/// cursor on either reveals the whole thing. An ATX heading is always
/// single-line (`line == last_line`), so this is bit-identical to the old
/// `any_on_line(line)` check there.
#[derive(Clone, Debug)]
pub struct HeadingM {
    pub sm: RevealSm,
    pub level: u8,
    pub line: usize,
    /// The heading's own last physical line: itself for an ATX heading, the
    /// underline's line for a setext heading.
    pub last_line: usize,
    pub range: ByteRange,
    /// Comrak's own `NodeHeading::setext` flag, carried forward verbatim —
    /// the ONE reliable "is this heading setext" signal. `underline` looks
    /// like the same signal but isn't: a defensive guard can null it out on
    /// a genuinely setext heading (see that field's docs), so any code that
    /// needs to know setext-vs-ATX, rather than "may I conceal a row",
    /// reads this field instead.
    pub setext: bool,
    /// The `"## "`-style prefix range (parent/child sourcepos-gap derivation,
    /// plan Context "Parse"). Empty for a setext heading (no leading prefix
    /// on its own text line).
    pub marker: ByteRange,
    /// The setext underline's own row, widened past `content_lines`'s last
    /// entry to also cover any enclosing depth's prefix nothing else will
    /// ever hide (a list item's fixed continuation indent, unlike a
    /// blockquote's own independently concealed `"> "`) — `None` for an
    /// ATX heading, and also `None` when a defensive guard degrades a
    /// genuinely setext heading (see `underline_of_setext_heading`'s
    /// docs). This is a conceal-eligibility signal, not a setext-ness
    /// signal — use `setext` for that.
    pub underline: Option<ByteRange>,
    pub inlines: Vec<Inline>,
    /// One entry per physical line `range` spans, each already clamped to
    /// that line's own `hint`-derived content start (parse time, container-
    /// aware) — the Revealed (raw-markup-shown) emit path iterates this
    /// instead of pushing `range` whole through the generic per-line
    /// splitter. An ATX heading is always single-line, so this is simply
    /// `vec![range]`; a setext heading spans its text line AND its `===`/
    /// `---` underline, and `range` alone — one contiguous span — cannot
    /// exclude an interior container prefix (a blockquote's repeating
    /// `"> "` on the underline's own continuation line) the SAME way
    /// `CodeFenceM::content_lines` already can't be replaced by
    /// `CodeFenceM::range` for a multi-line fence (mirrors that fix).
    pub content_lines: Vec<ByteRange>,
}

impl HeadingM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let (first, last) = (self.line, self.last_line);
        let want = ctx.grant.resolve(|| ctx.cursors.any_in_lines(first, last));
        let mut dirty = self.sm.transition(want);
        let child_ctx = ctx.child(self.sm.state());
        for c in &mut self.inlines {
            dirty |= c.sync(&child_ctx);
        }
        dirty
    }
}

/// One `"> "` marker on one line of a blockquote. Blockquote reveal is
/// per-line (the reveal-policy table: "Blockquote (per line)") — each line's
/// marker conceals/reveals independently of its siblings, so the marker
/// array (not a single `RevealSm`) is the concealable unit here.
#[derive(Clone, Debug)]
pub struct BlockquoteMarkerM {
    pub sm: RevealSm,
    pub line: usize,
    pub marker: ByteRange,
}

#[derive(Clone, Debug)]
pub struct BlockquoteM {
    pub range: ByteRange,
    pub markers: Vec<BlockquoteMarkerM>,
    pub children: Vec<Block>,
}

impl BlockquoteM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let mut dirty = false;
        for m in &mut self.markers {
            let line = m.line;
            let want = ctx.grant.resolve(|| ctx.cursors.any_on_line(line));
            dirty |= m.sm.transition(want);
        }
        // The ">" marker is a per-line decoration, not a wrapping conceal
        // unit over the quoted content — nested blocks see the same grant
        // the blockquote itself received (an unfocused/forced-concealed
        // parent still forces through; a focused blockquote does not itself
        // force its content revealed just because one line's marker is).
        let child_ctx = InheritCtx { ..*ctx };
        for c in &mut self.children {
            dirty |= c.sync(&child_ctx);
        }
        dirty
    }

    fn reveal_state(&self) -> RevealState {
        if self
            .markers
            .iter()
            .any(|m| m.sm.state() == RevealState::Revealed)
        {
            RevealState::Revealed
        } else {
            RevealState::Rendered
        }
    }
}

/// A fenced code block. Decide policy: `cursors.any_in_lines(first, last)` —
/// the whole block reveals as a unit.
///
/// `content_lines` is one `ByteRange` PER content line, not a single
/// contiguous range: when the fence sits inside a container (blockquote,
/// list item), every content line past the first can carry its own
/// repeating container prefix (`"> "`) that a single contiguous range
/// could never exclude — the per-line decomposition is what lets each
/// line's range start AFTER that line's own container prefix instead of
/// re-claiming bytes the container already hid (the class of bug this
/// type shape exists to make unrepresentable; see `parse::block`'s
/// `CodeBlock` arm).
#[derive(Clone, Debug)]
pub struct CodeFenceM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub first_line: usize,
    pub last_line: usize,
    pub language: String,
    pub fence_open: ByteRange,
    /// Absent while the fence is unterminated — the shape every fence has
    /// as it is being typed — in which case every line past the opening
    /// one is content.
    pub fence_close: Option<ByteRange>,
    pub content_lines: Vec<ByteRange>,
}

impl CodeFenceM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let (first, last) = (self.first_line, self.last_line);
        let want = ctx.grant.resolve(|| ctx.cursors.any_in_lines(first, last));
        self.sm.transition(want)
    }
}

/// A list item's marker (`"- "`, `"1. "`, `"- [x] "`) plus its nested block
/// content. Decide policy: `cursors.any_on_line(line)` (the item's first
/// line — the reveal-policy table's "ListItem/Task marker" row).
#[derive(Clone, Debug)]
pub struct ListItemM {
    pub sm: RevealSm,
    pub line: usize,
    pub marker: ByteRange,
    /// The `"[x]"`/`"[ ]"` char range within the marker, for task items.
    pub task: Option<ByteRange>,
    pub children: Vec<Block>,
}

impl ListItemM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let line = self.line;
        let want = ctx.grant.resolve(|| ctx.cursors.any_on_line(line));
        let mut dirty = self.sm.transition(want);
        let child_ctx = ctx.child(self.sm.state());
        for c in &mut self.children {
            dirty |= c.sync(&child_ctx);
        }
        dirty
    }
}

#[derive(Clone, Debug)]
pub struct ListM {
    pub ordered: bool,
    pub items: Vec<ListItemM>,
}

impl ListM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let mut dirty = false;
        for item in &mut self.items {
            dirty |= item.sync(ctx);
        }
        dirty
    }

    fn reveal_state(&self) -> RevealState {
        if self
            .items
            .iter()
            .any(|i| i.sm.state() == RevealState::Revealed)
        {
            RevealState::Revealed
        } else {
            RevealState::Rendered
        }
    }
}

/// A thematic break (`---`, `***`). Decide policy: `cursors.any_on_line`.
#[derive(Clone, Debug)]
pub struct HrM {
    pub sm: RevealSm,
    pub line: usize,
    pub range: ByteRange,
}

impl HrM {
    fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let line = self.line;
        let want = ctx.grant.resolve(|| ctx.cursors.any_on_line(line));
        self.sm.transition(want)
    }
}

/// YAML frontmatter, with its `---` delimiter lines tracked apart from the
/// body between them: the body is a code region, highlighted as YAML like
/// any fence's content, while the delimiters are not — a single contiguous
/// range could not tell the two apart. Pinned Revealed, ignoring
/// `ctx.grant` entirely (the reveal-policy table: "Frontmatter, Verbatim |
/// pinned Revealed (no Decide)"), so nothing here is ever concealed.
///
/// `content_lines` is one `ByteRange` per body line, never collapsed into a
/// single span — the same per-line shape, for the same reasons, as
/// `CodeFenceM::content_lines`.
#[derive(Clone, Debug)]
pub struct FrontmatterM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub first_line: usize,
    pub last_line: usize,
    pub open: ByteRange,
    /// Absent only for a degenerate single-line node, whose one line is
    /// already claimed as `open`.
    pub close: Option<ByteRange>,
    pub content_lines: Vec<ByteRange>,
}

impl FrontmatterM {
    fn sync(&mut self, _ctx: &InheritCtx) -> bool {
        self.sm.transition(RevealState::Revealed)
    }
}

/// Phase-1 token scope's catch-all for tables, HTML blocks, math blocks, and
/// any comrak node kind this crate doesn't otherwise model: raw passthrough,
/// pinned Revealed (plan: "unknown syntax degrades to visible raw text,
/// never lost").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerbatimKind {
    Table,
    Html,
    Math,
    /// An indented (non-fenced) code block. Verbatim like the rest, but it
    /// is the one variant that is genuinely CODE — `code_region::collect`
    /// keys off it to tell an indented code block apart from the other
    /// passthroughs, none of which may be treated as code.
    IndentedCode,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct VerbatimM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub kind: VerbatimKind,
    /// One entry per physical line `range` spans (AS COMRAK PARSED
    /// THEM), each already clamped to that line's own `hint`-derived
    /// content start — the emit path iterates this instead of pushing
    /// `range` whole through the generic per-line splitter (verification
    /// round 9 MAJOR: a comrak-recognized table/HTML block/unknown
    /// construct nested in a blockquote or list item is exactly as
    /// multi-line as a fence or a setext heading, and `range` alone —
    /// one contiguous span — can't exclude an interior container prefix
    /// any more than `CodeFenceM::range`/`HeadingM::range` can; this is
    /// the SAME shape those two were already fixed for, just never
    /// extended here). Always single-entry `vec![range]` for a
    /// genuinely single-line block.
    pub content_lines: Vec<ByteRange>,
}

impl VerbatimM {
    fn sync(&mut self, _ctx: &InheritCtx) -> bool {
        self.sm.transition(RevealState::Revealed)
    }
}

#[derive(Clone, Debug)]
pub enum Block {
    Paragraph(ParagraphM),
    Heading(HeadingM),
    Blockquote(BlockquoteM),
    CodeFence(CodeFenceM),
    List(ListM),
    ThematicBreak(HrM),
    Frontmatter(FrontmatterM),
    Verbatim(VerbatimM),
    Table(TableM),
}

impl Block {
    pub fn sync(&mut self, ctx: &InheritCtx) -> bool {
        match self {
            Block::Paragraph(m) => m.sync(ctx),
            Block::Heading(m) => m.sync(ctx),
            Block::Blockquote(m) => m.sync(ctx),
            Block::CodeFence(m) => m.sync(ctx),
            Block::List(m) => m.sync(ctx),
            Block::ThematicBreak(m) => m.sync(ctx),
            Block::Frontmatter(m) => m.sync(ctx),
            Block::Verbatim(m) => m.sync(ctx),
            Block::Table(m) => m.sync(ctx),
        }
    }

    /// The block's own reveal state, for tests and the emitter. Composite
    /// blocks (`Blockquote`, `List`) report `Revealed` iff any sub-marker is
    /// — `Paragraph` has no marker of its own so it reports `Rendered` (it
    /// is never itself a conceal target).
    pub fn reveal_state(&self) -> RevealState {
        match self {
            Block::Paragraph(_) => RevealState::Rendered,
            Block::Heading(m) => m.sm.state(),
            Block::Blockquote(m) => m.reveal_state(),
            Block::CodeFence(m) => m.sm.state(),
            Block::List(m) => m.reveal_state(),
            Block::ThematicBreak(m) => m.sm.state(),
            Block::Frontmatter(m) => m.sm.state(),
            Block::Verbatim(m) => m.sm.state(),
            Block::Table(m) => m.reveal_state(),
        }
    }

    pub fn range(&self) -> ByteRange {
        match self {
            Block::Paragraph(m) => m.range,
            Block::Heading(m) => m.range,
            Block::Blockquote(m) => m.range,
            Block::CodeFence(m) => m.range,
            Block::List(m) => {
                let first = m.items.first();
                let last = m.items.last();
                match (first, last) {
                    (Some(f), Some(l)) => {
                        let f_range = f.children.first().map_or(f.marker, |c| c.range());
                        let l_range = l.children.last().map_or(l.marker, |c| c.range());
                        ByteRange::new(
                            f.marker.start.min(f_range.start),
                            l_range.end.max(l.marker.end),
                        )
                    }
                    _ => ByteRange::default(),
                }
            }
            Block::ThematicBreak(m) => m.range,
            Block::Frontmatter(m) => m.range,
            Block::Verbatim(m) => m.range,
            Block::Table(m) => m.range,
        }
    }
}
