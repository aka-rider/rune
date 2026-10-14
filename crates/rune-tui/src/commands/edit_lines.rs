use std::ops::Range;

use rune_core::buffer::{Buffer, Edit};
use rune_core::coords::{BufferOffset, BufferPoint, VisualCol};
use rune_core::cursor::{Cursor, CursorId};
use rune_core::undo::EditKind;

use crate::app::App;
use crate::commands::edit_core::{apply_edit_batch_with_cursors, apply_edit_batch_with_cursors_on};
use crate::commands::line_blocks::touching_line_blocks;
use crate::document::DocumentId;

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

    let _ = apply_edit_batch_with_cursors(
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
    line: usize,
    col: usize,
}

pub fn delete_line(app: &mut App, id: DocumentId) {
    let Some(doc) = app.doc(id) else { return };
    let cursors_before = doc.cursors.clone();
    let buf = &doc.buffer;
    let line_count = buf.line_count();

    let mut infos: Vec<(Edit, CursorId)> = Vec::new();
    let mut landings: Vec<Landing> = Vec::new();
    let mut lines_removed = 0usize;
    for block in touching_line_blocks(cursors_before.all(), buf) {
        let Some(head) = block.cursors.first().copied() else {
            continue;
        };
        let reaches_last = block.lines.end >= line_count;
        let Some(range) = deleted_bytes(buf, &block.lines, reaches_last) else {
            continue;
        };
        let landing_line = if reaches_last {
            block.lines.start.saturating_sub(1)
        } else {
            block.lines.start
        };
        landings.push(Landing {
            head,
            line: landing_line.saturating_sub(lines_removed),
            col: buf.offset_to_line_col(head.position.get()).col,
        });
        lines_removed += block.lines.len();
        infos.push((
            Edit {
                start: range.start,
                end: range.end,
                insert: String::new(),
            },
            head.id,
        ));
    }

    let _ = apply_edit_batch_with_cursors_on(
        app,
        id,
        infos,
        &cursors_before,
        EditKind::Other,
        move |new_buf, _, _| {
            landings
                .iter()
                .map(|l| {
                    let at = BufferOffset(landing_offset(new_buf, l.line, l.col));
                    Cursor {
                        position: at,
                        anchor: at,
                        ..l.head
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

fn landing_offset(buf: &Buffer, line: usize, col: usize) -> usize {
    let content_len = buf
        .line_start(line)
        .zip(buf.line_content_end(line))
        .map_or(col, |(start, end)| end.saturating_sub(start));
    buf.line_col_to_offset(BufferPoint {
        line,
        col: col.min(content_len),
    })
}

#[cfg(test)]
#[path = "edit_lines_tests.rs"]
mod tests;
