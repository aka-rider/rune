use std::ops::Range;

use rune_core::buffer::{Buffer, Edit};
use rune_core::coords::{BufferOffset, BufferPoint};
use rune_core::cursor::{Cursor, CursorId, CursorSet};
use rune_core::undo::EditKind;

use crate::app::App;
use crate::commands::edit_core::apply_edit_batch_with_cursors_on;
use crate::commands::line_blocks::{LineBlock, overlapping_line_blocks, touching_line_blocks};
use crate::document::DocumentId;
use crate::messages;

struct Placed {
    cursor: Cursor,
    anchor: BufferPoint,
    position: BufferPoint,
    line_delta: isize,
}

impl Placed {
    fn new(cursor: Cursor, buf: &Buffer, line_delta: isize) -> Placed {
        Placed {
            cursor,
            anchor: buf.offset_to_line_col(cursor.anchor.get()),
            position: buf.offset_to_line_col(cursor.position.get()),
            line_delta,
        }
    }

    fn land(&self, new_buf: &Buffer) -> Cursor {
        let at = |p: BufferPoint| {
            BufferOffset(new_buf.line_col_to_offset(BufferPoint {
                line: p.line.saturating_add_signed(self.line_delta),
                col: p.col,
            }))
        };
        Cursor {
            anchor: at(self.anchor),
            position: at(self.position),
            ..self.cursor
        }
    }
}

enum Refusal {
    BufferLookup,
    LoneCarriageReturn(&'static str),
}

impl Refusal {
    fn report(self, app: &mut App) {
        match self {
            Refusal::BufferLookup => {
                messages::error(app, "line edit failed: a line fell outside the buffer")
            }
            Refusal::LoneCarriageReturn(verb) => messages::warn(
                app,
                format!("line ends with a lone carriage return; {verb} refused"),
            ),
        }
    }
}

fn found<T>(lookup: Option<T>) -> Result<T, Refusal> {
    lookup.ok_or(Refusal::BufferLookup)
}

struct Plan {
    edits: Vec<(Edit, CursorId)>,
    placed: Vec<Placed>,
}

fn apply_plan(app: &mut App, id: DocumentId, plan: Plan) {
    let Some(doc) = app.doc(id) else { return };
    let Plan { edits, placed } = plan;
    if edits.is_empty() {
        let landed: Vec<Cursor> = placed.iter().map(|p| p.land(&doc.buffer)).collect();
        if let Some(doc) = app.doc_mut(id) {
            doc.cursors = CursorSet::new_from(&landed);
        }
        return;
    }
    let cursors_before = doc.cursors.clone();
    apply_edit_batch_with_cursors_on(
        app,
        id,
        edits,
        &cursors_before,
        EditKind::Other,
        move |new_buf, _, _| placed.iter().map(|p| p.land(new_buf)).collect(),
    );
}

#[derive(Clone, Copy)]
enum SelectionStays {
    OnUpperCopy,
    OnLowerOriginal,
}

pub fn clone_line_up(app: &mut App, id: DocumentId) {
    clone_lines(app, id, SelectionStays::OnUpperCopy);
}

pub fn clone_line_down(app: &mut App, id: DocumentId) {
    clone_lines(app, id, SelectionStays::OnLowerOriginal);
}

fn clone_lines(app: &mut App, id: DocumentId, selection: SelectionStays) {
    let Some(doc) = app.doc(id) else { return };
    let blocks = overlapping_line_blocks(doc.cursors.all(), &doc.buffer);
    match plan_clones(&doc.buffer, &blocks, selection) {
        Ok(plan) => apply_plan(app, id, plan),
        Err(refusal) => refusal.report(app),
    }
}

fn plan_clones(
    buf: &Buffer,
    blocks: &[LineBlock],
    selection: SelectionStays,
) -> Result<Plan, Refusal> {
    let mut plan = Plan {
        edits: Vec::new(),
        placed: Vec::new(),
    };
    let mut inserted_before = 0;
    for block in blocks {
        let own = block.lines.len();
        let lines = match selection {
            SelectionStays::OnUpperCopy => inserted_before,
            SelectionStays::OnLowerOriginal => inserted_before + own,
        };
        let delta = lines.cast_signed();
        let owner = found(block.cursors.first())?;
        plan.edits.push((clone_edit(buf, &block.lines)?, owner.id));
        plan.placed
            .extend(block.cursors.iter().map(|c| Placed::new(*c, buf, delta)));
        inserted_before += own;
    }
    Ok(plan)
}

fn clone_edit(buf: &Buffer, lines: &Range<usize>) -> Result<Edit, Refusal> {
    let last = found(lines.end.checked_sub(1))?;
    let start = found(buf.line_start(lines.start))?;
    let terminator = found(buf.line_terminator_range(last))?;
    let own_terminator = found(buf.slice(terminator.start, terminator.end))?;
    let content_end = found(buf.line_content_end(last))?;
    let content = found(buf.slice(start, content_end))?;
    let terminator = if own_terminator.is_empty() {
        if content.ends_with('\r') {
            return Err(Refusal::LoneCarriageReturn("clone"));
        }
        buf.line_break_at(found(buf.line_start(last))?)
    } else {
        own_terminator
    };
    Ok(Edit {
        start,
        end: start,
        insert: format!("{content}{terminator}"),
    })
}

struct Shift {
    region: fn(&Range<usize>, usize) -> Option<Range<usize>>,
    rotate: fn(&mut [&str]),
    line_delta: isize,
}

const UP: Shift = Shift {
    region: |block, _| block.start.checked_sub(1).map(|start| start..block.end),
    rotate: |texts| texts.rotate_left(1),
    line_delta: -1,
};

const DOWN: Shift = Shift {
    region: |block, line_count| (block.end < line_count).then_some(block.start..block.end + 1),
    rotate: |texts| texts.rotate_right(1),
    line_delta: 1,
};

pub fn move_line_up(app: &mut App, id: DocumentId) {
    move_lines(app, id, &UP);
}

pub fn move_line_down(app: &mut App, id: DocumentId) {
    move_lines(app, id, &DOWN);
}

fn move_lines(app: &mut App, id: DocumentId, shift: &Shift) {
    let Some(doc) = app.doc(id) else { return };
    let blocks = touching_line_blocks(doc.cursors.all(), &doc.buffer);
    match plan_moves(&doc.buffer, &blocks, shift) {
        Ok(plan) => apply_plan(app, id, plan),
        Err(refusal) => refusal.report(app),
    }
}

fn plan_moves(buf: &Buffer, blocks: &[LineBlock], shift: &Shift) -> Result<Plan, Refusal> {
    let line_count = buf.line_count();
    let mut plan = Plan {
        edits: Vec::new(),
        placed: Vec::new(),
    };
    for block in blocks {
        let delta = match (shift.region)(&block.lines, line_count) {
            Some(region) => {
                let owner = found(block.cursors.first())?;
                let edit = rotated_lines_edit(buf, &region, shift.rotate)?;
                if buf.slice(edit.start, edit.end) != Some(edit.insert.as_str()) {
                    plan.edits.push((edit, owner.id));
                }
                shift.line_delta
            }
            None => 0,
        };
        plan.placed
            .extend(block.cursors.iter().map(|c| Placed::new(*c, buf, delta)));
    }
    Ok(plan)
}

fn rotated_lines_edit(
    buf: &Buffer,
    region: &Range<usize>,
    rotate: fn(&mut [&str]),
) -> Result<Edit, Refusal> {
    let mut texts = Vec::new();
    let mut terminators = Vec::new();
    for line in region.clone() {
        let content_end = found(buf.line_content_end(line))?;
        texts.push(found(buf.slice(found(buf.line_start(line))?, content_end))?);
        let terminator = found(buf.line_terminator_range(line))?;
        terminators.push(found(buf.slice(terminator.start, terminator.end))?);
    }
    rotate(&mut texts);
    if texts
        .iter()
        .zip(&terminators)
        .any(|(text, terminator)| text.ends_with('\r') && terminator.starts_with('\n'))
    {
        return Err(Refusal::LoneCarriageReturn("move"));
    }
    let last = found(region.end.checked_sub(1))?;
    Ok(Edit {
        start: found(buf.line_start(region.start))?,
        end: found(buf.line_terminator_range(last))?.end,
        insert: texts
            .iter()
            .zip(&terminators)
            .map(|(text, terminator)| format!("{text}{terminator}"))
            .collect(),
    })
}

#[cfg(test)]
#[path = "edit_lines_move_tests.rs"]
mod tests;
