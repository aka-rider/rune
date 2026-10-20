use std::time::SystemTime;

use rusqlite::{OptionalExtension, Transaction, params};

use rune_core::buffer::{Buffer, clamp_to_char_boundary};
use rune_core::coords::BufferOffset;
use rune_core::cursor::Cursor;
use rune_core::undo::{Step, reapply_verified};

use crate::Error;
use crate::ids::{DocId, Seq, SessionId};
use crate::journal::{self, EditRow};
use crate::snapshot::{self, Anchor};

const MAX_STEPS: i64 = 1000;

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
    Unreadable,
    PositionRepaired,
    RedoTailDropped,
}

impl Lineage {
    pub fn position_seq(&self) -> Seq {
        self.done.last().map_or(self.base_seq, |s| s.seq)
    }

    pub fn seqs(&self) -> impl Iterator<Item = Seq> + '_ {
        self.done.iter().chain(&self.undone).map(|s| s.seq)
    }
}

struct Floor {
    seq: Seq,
    content: String,
}

pub(crate) struct Replay {
    pub(crate) lineage: Lineage,
    pub(crate) loss: Option<HistoryLoss>,
    floor: Option<Floor>,
}

impl Replay {
    fn lost(position: Seq, loss: HistoryLoss) -> Replay {
        Replay {
            lineage: Lineage {
                base_seq: position,
                ..Lineage::default()
            },
            loss: Some(loss),
            floor: None,
        }
    }
}

struct Window {
    anchor: Anchor,
    floor_seq: Option<Seq>,
    rows: Vec<EditRow>,
}

/// The lineage of `session_id`'s `doc_id` for a tab that shows `shown`. A
/// history that is lost is lost once: a rebase snapshot of `shown` is
/// written at the current position, so the next read starts there instead
/// of failing on the same rows again. No row is deleted, and the snapshot is
/// written only when the journal itself recovers `shown` at that position,
/// so it can never change what recovery returns.
pub(crate) fn read_lineage(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    shown: &str,
    now: SystemTime,
) -> Result<(Lineage, Option<HistoryLoss>), Error> {
    let Replay { lineage, loss, .. } = replay(tx, session_id, doc_id, shown)?;
    if loss.is_some() {
        rebase_at(tx, session_id, doc_id, shown, lineage.position_seq(), now)?;
    }
    Ok((lineage, loss))
}

fn rebase_at(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    shown: &str,
    position: Seq,
    now: SystemTime,
) -> Result<(), Error> {
    if snapshot::has_rebase_at(tx, session_id, doc_id, position)? {
        return Ok(());
    }
    let journal_agrees = match snapshot::recover_document(tx, session_id, doc_id) {
        Ok(recovered) => recovered.content == shown,
        Err(e) if e.is_stored_data_damage() => false,
        Err(e) => return Err(e),
    };
    if journal_agrees {
        snapshot::create_rebase_snapshot(tx, session_id, now, doc_id, shown, position)?;
    }
    Ok(())
}

/// Reads the durable history of `session_id`'s `doc_id` as a replayable
/// lineage, proving that the content at the undo position is `shown`. A
/// pure read: it never deletes or rewrites a row, because a live binding
/// elsewhere may already number its undo positions against those rows. A
/// history that cannot be read or does not replay is reported as a loss
/// and replaced by an empty lineage at the current position; it is never
/// pruned. The replay starts at the newest rebase snapshot, so rows below
/// one are never read.
pub(crate) fn replay(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    shown: &str,
) -> Result<Replay, Error> {
    let position = journal::current_seq(tx, session_id, doc_id)?;
    let window = match read_window(tx, session_id, doc_id, position) {
        Ok(Some(window)) => window,
        Ok(None) => return Ok(Replay::lost(position, HistoryLoss::Unreadable)),
        Err(e) if e.is_stored_data_damage() => {
            return Ok(Replay::lost(position, HistoryLoss::Unreadable));
        }
        Err(e) => return Err(e),
    };
    Ok(fold(window, position, shown)
        .unwrap_or_else(|| Replay::lost(position, HistoryLoss::Unreplayable)))
}

/// Deletes the steps `replay` kept out of the lineage because of the cap,
/// leaving a snapshot of the content at the new floor. Only a session that
/// no binding has numbered positions against yet may call this: the rows it
/// deletes are rows a live token could still map an undo onto.
pub(crate) fn trim_to_cap(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    replay: &Replay,
    now: SystemTime,
) -> Result<(), Error> {
    let Some(floor) = &replay.floor else {
        return Ok(());
    };
    tx.execute(
        "DELETE FROM events WHERE doc_id=?1 AND session_id=?2 AND seq <= ?3",
        params![doc_id, session_id, floor.seq],
    )?;
    tx.execute(
        "DELETE FROM snapshots WHERE doc_id=?1 AND session_id=?2 AND seq <= ?3",
        params![doc_id, session_id, floor.seq],
    )?;
    snapshot::create_snapshot(tx, session_id, now, doc_id, &floor.content, floor.seq)?;
    Ok(())
}

/// The seq of the newest row the cap trims, if any. Never reaches past the
/// undo position: a step at or after it is the user's redo tail or current
/// state, not old history.
fn capped_floor(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    position: Seq,
    base: Seq,
) -> Result<Option<Seq>, Error> {
    let oldest_kept_candidate: Option<Seq> = tx
        .query_row(
            "SELECT seq FROM events WHERE doc_id=?1 AND session_id=?2 \
             AND seq > ?4 \
             ORDER BY seq DESC LIMIT 1 OFFSET ?3",
            params![doc_id, session_id, MAX_STEPS, base],
            |r| r.get(0),
        )
        .optional()?;
    Ok(oldest_kept_candidate.map(|seq| seq.min(position)))
}

fn read_window(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    position: Seq,
) -> Result<Option<Window>, Error> {
    let base = match snapshot::newest_rebase_anchor_at(tx, session_id, doc_id, position)? {
        Some(rebase) => Some(rebase),
        None => snapshot::oldest_anchor(tx, session_id, doc_id)?,
    };
    let floor_seq = capped_floor(
        tx,
        session_id,
        doc_id,
        position,
        base.as_ref().map_or(Seq(0), |anchor| anchor.seq),
    )?;
    let anchor = match floor_seq {
        Some(floor) => snapshot::newest_anchor_at(tx, session_id, doc_id, floor)?,
        None => Some(base.unwrap_or(Anchor {
            seq: Seq(0),
            content: String::new(),
        })),
    };
    let Some(anchor) = anchor else {
        return Ok(None);
    };
    let rows = journal::edits_in_range(tx, session_id, doc_id, anchor.seq, Seq(i64::MAX))?;
    Ok(Some(Window {
        anchor,
        floor_seq,
        rows,
    }))
}

fn fold(window: Window, position: Seq, shown: &str) -> Option<Replay> {
    let Window {
        anchor,
        floor_seq,
        rows,
    } = window;
    let dropped = floor_seq.map_or(0, |floor| rows.partition_point(|row| row.seq <= floor));
    let mut kept = rows;
    let older = kept.drain(..dropped);
    let mut buf = Buffer::new(anchor.content);
    for row in older {
        buf = reapply_verified(&buf, &row.edits).ok()?;
    }
    let base_seq = floor_seq.unwrap_or(anchor.seq);
    let floor = floor_seq.map(|seq| Floor {
        seq,
        content: buf.content().to_string(),
    });

    let done_count = kept.partition_point(|row| row.seq <= position);
    let mut at_position = (done_count == 0).then(|| buf.content().to_string());
    let mut steps = Vec::with_capacity(kept.len());
    for (i, row) in kept.into_iter().enumerate() {
        let cursors_before = clamp_cursors(buf.content(), row.cursors_before);
        buf = reapply_verified(&buf, &row.edits).ok()?;
        let cursors_after = clamp_cursors(buf.content(), row.cursors_after);
        steps.push(LineageStep {
            seq: row.seq,
            step: Step {
                edits: row.edits,
                cursors_before,
                cursors_after,
                kind: row.kind,
            },
        });
        if i + 1 == done_count {
            at_position = Some(buf.content().to_string());
        }
    }
    if at_position.as_deref() != Some(shown) {
        return None;
    }
    let undone = steps.split_off(done_count);
    Some(Replay {
        lineage: Lineage {
            base_seq,
            done: steps,
            undone,
        },
        loss: None,
        floor,
    })
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

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
