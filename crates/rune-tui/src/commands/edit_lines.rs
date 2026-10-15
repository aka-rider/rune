use std::ops::Range;

use rune_core::buffer::{Buffer, Edit};
use rune_core::coords::{BufferOffset, BufferPoint, VisualCol};
use rune_core::cursor::{Cursor, CursorId};
use rune_core::undo::EditKind;
use rune_md::element::doc::ViewSnapshots;

use crate::app::App;
use crate::commands::edit_core::{apply_edit_batch_with_cursors, apply_edit_batch_with_cursors_on};
use crate::commands::line_blocks::touching_line_blocks;
use crate::commands::multi::visual_col_on_line;
use crate::commands::nav::update_horizontal;
use crate::document::DocumentId;
use crate::keymap::Extend;
use crate::messages;

struct LineShift {
    start: usize,
    removed: usize,
    inserted: usize,
}

fn shift_through(offset: usize, shifts: &[LineShift]) -> usize {
    let mut out = offset;
    for s in shifts {
        if offset <= s.start {
            break;
        }
        if offset < s.start.saturating_add(s.removed) {
            out = out.saturating_sub(offset.saturating_sub(s.start));
            break;
        }
        out = out.saturating_sub(s.removed).saturating_add(s.inserted);
    }
    out
}

fn per_selected_line_edits(
    app: &mut App,
    id: DocumentId,
    build: impl Fn(usize, &Buffer) -> Option<Edit>,
) {
    let Some(doc) = app.doc(id) else { return };
    let cursors_before = doc.cursors.clone();
    let before = cursors_before.all().to_vec();

    let mut infos: Vec<(Edit, CursorId)> = Vec::new();
    for block in touching_line_blocks(&before, &doc.buffer) {
        let Some(owner) = block.cursors.first() else {
            continue;
        };
        for line in block.lines {
            if let Some(edit) = build(line, &doc.buffer) {
                infos.push((edit, owner.id));
            }
        }
    }

    let mut shifts: Vec<LineShift> = infos
        .iter()
        .map(|(e, _)| LineShift {
            start: e.start,
            removed: e.end.saturating_sub(e.start),
            inserted: e.insert.len(),
        })
        .collect();
    shifts.sort_by_key(|s| s.start);

    apply_edit_batch_with_cursors(
        app,
        id,
        infos,
        &cursors_before,
        EditKind::Other,
        move |_, _| {
            before
                .iter()
                .map(|c| Cursor {
                    position: BufferOffset(shift_through(c.position.get(), &shifts)),
                    anchor: BufferOffset(shift_through(c.anchor.get(), &shifts)),
                    desired_col: VisualCol(0),
                    id: c.id,
                })
                .collect()
        },
    );
}

pub fn indent(app: &mut App, id: DocumentId) {
    per_selected_line_edits(app, id, |line, buf| {
        let line_start = buf.line_start(line)?;
        Some(Edit {
            start: line_start,
            end: line_start,
            insert: "\t".to_string(),
        })
    });
}

pub fn outdent(app: &mut App, id: DocumentId) {
    per_selected_line_edits(app, id, dedent_edit_for_line);
}

fn dedent_edit_for_line(line: usize, buf: &Buffer) -> Option<Edit> {
    let line_start = buf.line_start(line)?;
    let line_end = buf.line_end(line)?;
    let line_text = buf.slice(line_start, line_end).unwrap_or("");

    let mut indent_end = 0usize;
    for ch in line_text.chars() {
        if ch == '\t' || ch == ' ' {
            indent_end += ch.len_utf8();
        } else {
            break;
        }
    }
    if indent_end == 0 {
        return None;
    }

    let mut remove = 1usize;
    if indent_end >= 4 {
        let space_run = line_text.bytes().take_while(|&b| b == b' ').count();
        if space_run >= 4 {
            remove = 4;
        }
    }
    if indent_end < remove {
        remove = indent_end;
    }

    Some(Edit {
        start: line_start,
        end: line_start + remove,
        insert: String::new(),
    })
}

struct Landing {
    head: Cursor,
    point: BufferPoint,
    desired_col: VisualCol,
}

struct Deletion {
    edits: Vec<(Edit, CursorId)>,
    landings: Vec<Landing>,
}

fn plan_deletion(cursors: &[Cursor], buf: &Buffer, view: &ViewSnapshots) -> Option<Deletion> {
    let line_count = buf.line_count();
    let mut plan = Deletion {
        edits: Vec::new(),
        landings: Vec::new(),
    };
    let mut lines_removed = 0usize;
    for block in touching_line_blocks(cursors, buf) {
        let head = block.cursors.first().copied()?;
        let reaches_last = block.lines.end >= line_count;
        let range = deleted_bytes(buf, &block.lines, reaches_last)?;
        let (landing_line, landing_line_after) = if reaches_last {
            let above = block.lines.start.saturating_sub(1);
            (above, above.saturating_sub(lines_removed))
        } else {
            (
                block.lines.end,
                block.lines.start.saturating_sub(lines_removed),
            )
        };
        let desired_col =
            update_horizontal(view, buf, head, head.position.get(), Extend::No).desired_col;
        let target = visual_col_on_line(view, buf, landing_line, desired_col);
        plan.landings.push(Landing {
            head,
            point: BufferPoint {
                line: landing_line_after,
                col: target.col,
            },
            desired_col,
        });
        lines_removed += block.lines.len();
        plan.edits.push((
            Edit {
                start: range.start,
                end: range.end,
                insert: String::new(),
            },
            head.id,
        ));
    }
    Some(plan)
}

pub fn delete_line(app: &mut App, id: DocumentId) {
    let Some(doc) = app.doc_mut(id) else { return };
    let view = doc.view();
    let cursors_before = doc.cursors.clone();
    let Some(Deletion { edits, landings }) =
        plan_deletion(cursors_before.all(), &doc.buffer, &view)
    else {
        messages::error(app, "delete line failed: a line fell outside the buffer");
        return;
    };
    apply_edit_batch_with_cursors_on(
        app,
        id,
        edits,
        &cursors_before,
        EditKind::Other,
        move |new_buf, _, _| {
            landings
                .iter()
                .map(|l| {
                    let at = BufferOffset(landing_offset(new_buf, l.point));
                    Cursor {
                        position: at,
                        anchor: at,
                        desired_col: l.desired_col,
                        id: l.head.id,
                    }
                })
                .collect()
        },
    );
}

fn deleted_bytes(buf: &Buffer, lines: &Range<usize>, reaches_last: bool) -> Option<Range<usize>> {
    match (lines.start, reaches_last) {
        (0, true) => Some(0..buf.len()),
        (above, true) => Some(buf.line_content_end(above - 1)?..buf.len()),
        (first, false) => Some(buf.line_start(first)?..buf.line_start(lines.end)?),
    }
}

fn landing_offset(buf: &Buffer, point: BufferPoint) -> usize {
    let content_len = buf
        .line_start(point.line)
        .zip(buf.line_content_end(point.line))
        .map_or(point.col, |(start, end)| end.saturating_sub(start));
    buf.line_col_to_offset(BufferPoint {
        line: point.line,
        col: point.col.min(content_len),
    })
}

#[cfg(test)]
#[path = "edit_lines_tests.rs"]
mod tests;
