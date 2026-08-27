use crate::element::inline::Inline;
use crate::element::table::TableM;
use rune_syntax::element::{ByteRange, InheritCtx, RevealSm, RevealState};

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

#[derive(Clone, Debug)]
pub struct HeadingM {
    pub sm: RevealSm,
    pub level: u8,
    pub line: usize,
    pub last_line: usize,
    pub range: ByteRange,
    pub setext: bool,
    pub marker: ByteRange,
    pub underline: Option<ByteRange>,
    pub inlines: Vec<Inline>,
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

#[derive(Clone, Debug)]
pub struct CodeFenceM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub first_line: usize,
    pub last_line: usize,
    pub language: String,
    pub fence_open: ByteRange,
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

#[derive(Clone, Debug)]
pub struct ListItemM {
    pub sm: RevealSm,
    pub line: usize,
    pub marker: ByteRange,
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

#[derive(Clone, Debug)]
pub struct FrontmatterM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub first_line: usize,
    pub last_line: usize,
    pub open: ByteRange,
    pub close: Option<ByteRange>,
    pub content_lines: Vec<ByteRange>,
}

impl FrontmatterM {
    fn sync(&mut self, _ctx: &InheritCtx) -> bool {
        self.sm.transition(RevealState::Revealed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerbatimKind {
    Table,
    Html,
    Math,
    IndentedCode,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct VerbatimM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub kind: VerbatimKind,
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
                        let f_range = f.children.first().map_or(f.marker, Block::range);
                        let l_range = l.children.last().map_or(l.marker, Block::range);
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

#[cfg(test)]
#[path = "block_tests.rs"]
mod tests;
