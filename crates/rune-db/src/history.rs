use std::time::SystemTime;

use rusqlite::{OptionalExtension, Transaction, params};

use rune_core::buffer::{AppliedEdit, clamp_to_char_boundary};
use rune_core::coords::BufferOffset;
use rune_core::cursor::Cursor;
use rune_core::undo::Step;

use crate::Error;
use crate::ids::{DocId, Seq, SessionId};
use crate::journal::{self, EditRow};
use crate::snapshot;

const MAX_STEPS: usize = 1000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineageStep {
    pub seq: Seq,
    pub step: Step,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lineage {
    pub base_seq: Seq,
    pub done: Vec<LineageStep>,
    pub undone: Vec<LineageStep>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryLoss {
    Unreplayable,
}

impl Lineage {
    pub fn position_seq(&self) -> Seq {
        self.done.last().map_or(self.base_seq, |s| s.seq)
    }

    pub fn head_seq(&self) -> Seq {
        self.undone.last().map_or(self.position_seq(), |s| s.seq)
    }

    pub fn seqs(&self) -> impl Iterator<Item = Seq> + '_ {
        self.done.iter().chain(&self.undone).map(|s| s.seq)
    }
}

struct Replayed {
    steps: Vec<LineageStep>,
    floor: Option<String>,
    at_position: String,
}

pub(crate) fn read_lineage(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    shown: &str,
    now: SystemTime,
) -> Result<(Lineage, Option<HistoryLoss>), Error> {
    let position = journal::current_seq(tx, session_id, doc_id)?;
    let (base_seq, base_content) = oldest_snapshot(tx, session_id, doc_id)?;
    let rows = journal::edits_in_range(tx, session_id, doc_id, base_seq, Seq(i64::MAX))?;
    let done_count = rows.partition_point(|row| row.seq <= position);
    let dropped = rows.len().saturating_sub(MAX_STEPS).min(done_count);

    let verified = match replay(base_content, rows, dropped, done_count) {
        Ok(replayed) if replayed.at_position == shown => Some(replayed),
        Ok(_) | Err(HistoryLoss::Unreplayable) => None,
    };
    let Some(replayed) = verified else {
        reset_to(tx, session_id, doc_id, position, shown, now)?;
        let lineage = Lineage {
            base_seq: position,
            ..Lineage::default()
        };
        return Ok((lineage, Some(HistoryLoss::Unreplayable)));
    };

    let mut older = replayed.steps;
    let mut done = older.split_off(dropped);
    let undone = done.split_off(done_count - dropped);
    let base_seq = older.last().map_or(base_seq, |s| s.seq);
    if let Some(floor) = replayed.floor {
        prune_before(tx, session_id, doc_id, base_seq, &floor, now)?;
    }
    Ok((
        Lineage {
            base_seq,
            done,
            undone,
        },
        None,
    ))
}

fn oldest_snapshot(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
) -> Result<(Seq, String), Error> {
    let oldest: Option<(Seq, String)> = tx
        .query_row(
            "SELECT seq, blob_hash FROM snapshots WHERE doc_id=?1 AND session_id=?2 \
             ORDER BY seq ASC, id ASC LIMIT 1",
            params![doc_id, session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    match oldest {
        Some((seq, hash)) => Ok((seq, snapshot::snapshot_content(tx, doc_id, hash)?)),
        None => Ok((Seq(0), String::new())),
    }
}

fn replay(
    base: String,
    rows: Vec<EditRow>,
    dropped: usize,
    done_count: usize,
) -> Result<Replayed, HistoryLoss> {
    let mut content = base;
    let mut steps = Vec::with_capacity(rows.len());
    let mut floor = None;
    let mut at_position = if done_count == 0 {
        content.clone()
    } else {
        String::new()
    };
    for (i, row) in rows.into_iter().enumerate() {
        let cursors_before = clamp_cursors(&content, row.cursors_before);
        apply_row(&mut content, &row.edits)?;
        let cursors_after = clamp_cursors(&content, row.cursors_after);
        steps.push(LineageStep {
            seq: row.seq,
            step: Step {
                edits: row.edits,
                cursors_before,
                cursors_after,
                kind: row.kind,
            },
        });
        if i + 1 == dropped {
            floor = Some(content.clone());
        }
        if i + 1 == done_count {
            at_position = content.clone();
        }
    }
    Ok(Replayed {
        steps,
        floor,
        at_position,
    })
}

fn apply_row(content: &mut String, edits: &[AppliedEdit]) -> Result<(), HistoryLoss> {
    let mut ordered: Vec<&AppliedEdit> = edits.iter().collect();
    ordered.sort_by_key(|edit| edit.start);
    if ordered
        .windows(2)
        .any(|pair| matches!(pair, [a, b] if a.start == b.start))
    {
        return Err(HistoryLoss::Unreplayable);
    }
    for edit in ordered {
        let end = edit
            .start
            .checked_add(edit.deleted.len())
            .ok_or(HistoryLoss::Unreplayable)?;
        if content.get(edit.start..end) != Some(edit.deleted.as_str()) {
            return Err(HistoryLoss::Unreplayable);
        }
        content.replace_range(edit.start..end, &edit.insert);
    }
    Ok(())
}

fn clamp_cursors(content: &str, cursors: Vec<Cursor>) -> Vec<Cursor> {
    let clamp = |offset: BufferOffset| BufferOffset(clamp_to_char_boundary(content, offset.get()));
    cursors
        .into_iter()
        .map(|cursor| Cursor {
            position: clamp(cursor.position),
            anchor: clamp(cursor.anchor),
            ..cursor
        })
        .collect()
}

fn prune_before(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    floor: Seq,
    content: &str,
    now: SystemTime,
) -> Result<(), Error> {
    tx.execute(
        "DELETE FROM events WHERE doc_id=?1 AND session_id=?2 AND seq <= ?3",
        params![doc_id, session_id, floor],
    )?;
    tx.execute(
        "DELETE FROM snapshots WHERE doc_id=?1 AND session_id=?2 AND seq <= ?3",
        params![doc_id, session_id, floor],
    )?;
    snapshot::create_snapshot(tx, session_id, now, doc_id, content, floor)?;
    Ok(())
}

fn reset_to(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    position: Seq,
    content: &str,
    now: SystemTime,
) -> Result<(), Error> {
    tx.execute(
        "DELETE FROM events WHERE doc_id=?1 AND session_id=?2 AND seq > ?3",
        params![doc_id, session_id, position],
    )?;
    prune_before(tx, session_id, doc_id, position, content, now)?;
    journal::move_undo_pos(tx, session_id, doc_id, position)
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
