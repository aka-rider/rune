use crate::element::inline::Inline;
use rune_syntax::element::{ByteRange, InheritCtx, RevealSm, RevealState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableAlign {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug)]
pub struct TableCellM {
    pub range: ByteRange,
    pub inlines: Vec<Inline>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableRowShape {
    Exact,
    Padded,
    Truncated,
}

#[derive(Clone, Debug)]
pub struct TableRowM {
    pub line: usize,
    pub is_header: bool,
    pub shape: TableRowShape,
    pub cells: Vec<TableCellM>,
}

#[derive(Clone, Debug)]
pub struct TableM {
    pub sm: RevealSm,
    pub range: ByteRange,
    pub aligns: Vec<TableAlign>,
    pub rows: Vec<TableRowM>,
    pub sep_line: usize,
    pub first_line: usize,
    pub last_line: usize,
    pub content_lines: Vec<ByteRange>,
}

impl TableM {
    pub(crate) fn sync(&mut self, ctx: &InheritCtx) -> bool {
        let (first, last) = (self.first_line, self.last_line);
        let want = ctx.grant.resolve(|| ctx.cursors.any_in_lines(first, last));
        let mut dirty = self.sm.transition(want);
        let child_ctx = ctx.child(self.sm.state());
        for row in &mut self.rows {
            for cell in &mut row.cells {
                for inline in &mut cell.inlines {
                    dirty |= inline.sync(&child_ctx);
                }
            }
        }
        dirty
    }

    pub(crate) fn reveal_state(&self) -> RevealState {
        self.sm.state()
    }
}
