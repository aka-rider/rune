//! The durable undo/redo journal: `append_edit`, `undo_peek`/`redo_peek`/
//! `move_undo_pos`, `current_seq`, and `edits_in_range`. `doc_id` alone is
//! always both the journal key and the recovery/undo unit; every query here
//! additionally scopes to `(doc_id, session_id)` together, so a DIFFERENT
//! session sharing this `doc_id` (two rune windows on the same file) can
//! never see or truncate this session's own events.
//!
//! Every function below takes an already-open `&Transaction`/`&Connection`
//! rather than a `Store` — the writer thread (`writer.rs`) is the one
//! caller allowed to hold the single read-write connection (plan decision
//! 7), and every write here is meant to run inside `retry::with_retry`'s
//! `BEGIN IMMEDIATE` (plan Gotchas). `now` is threaded in explicitly rather
//! than read from a clock here, so the caller's injected clock (plan
//! Gotchas: "rune-db must take a `clock: ... -> SystemTime` injection") is
//! the only place wall-clock nondeterminism can enter, sampled ONCE per
//! `append_edit` call for the row's `at` timestamp.

#[cfg(feature = "test-support")]
use rusqlite::OptionalExtension;
use rusqlite::{Transaction, params};

use rune_core::buffer::AppliedEdit;
#[cfg(feature = "test-support")]
use rune_core::cursor::Cursor;

use crate::Error;
use crate::ids::{DocId, Seq, SessionId};
#[cfg(feature = "test-support")]
use crate::payload::cursors_from_json;
use crate::payload::edits_from_json;

pub use crate::journal_append::append_edit;

/// One undo/redo journal step: the edits to (re)apply to the buffer, the
/// cursor state to restore, and the journal position `move_undo_pos` should
/// commit to once the buffer reapply succeeds.
#[cfg(feature = "test-support")]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub edits: Vec<AppliedEdit>,
    pub cursors: Vec<Cursor>,
    pub new_pos: Seq,
}

/// One edit row tagged with the journal seq it was recorded at.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EditRow {
    pub seq: Seq,
    pub edits: Vec<AppliedEdit>,
}

/// Returns the most recent event at or before `session_id`'s current
/// journal position for `doc_id`, plus the position the journal should move
/// to (one behind that event) once the buffer edit applies. READ-ONLY: does
/// NOT mutate `current_seq` — the caller commits via `move_undo_pos` only
/// after the buffer reapply succeeds. `Ok(None)` means genuinely
/// nothing to undo; `Err` means the read or an event's payload was corrupt
/// — surfaced, never folded into `Ok(None)`.
#[cfg(feature = "test-support")]
pub fn undo_peek(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
) -> Result<Option<Step>, Error> {
    let position = current_seq(tx, session_id, doc_id)?;

    let row: Option<(i64, String, String)> = tx
        .query_row(
            "SELECT seq, edits, cursors_before FROM events \
             WHERE doc_id=?1 AND session_id=?2 AND seq<=?3 ORDER BY seq DESC LIMIT 1",
            params![doc_id, session_id, position],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((seq, edits_json, cursors_json)) = row else {
        return Ok(None);
    };

    let edits = edits_from_json(&edits_json)?;
    let cursors = cursors_from_json(&cursors_json)?;
    Ok(Some(Step {
        edits,
        cursors,
        new_pos: Seq(seq - 1),
    }))
}

/// Returns the next event after `session_id`'s current journal position for
/// `doc_id`, plus the position the journal should advance to. Mirrors
/// `undo_peek`: READ-ONLY, `Ok(None)` means genuinely nothing to redo, a
/// corrupt payload is `Err`, never folded into `Ok(None)`.
#[cfg(feature = "test-support")]
pub fn redo_peek(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
) -> Result<Option<Step>, Error> {
    let position = current_seq(tx, session_id, doc_id)?;

    let row: Option<(i64, String, String)> = tx
        .query_row(
            "SELECT seq, edits, cursors_after FROM events \
             WHERE doc_id=?1 AND session_id=?2 AND seq>?3 ORDER BY seq ASC LIMIT 1",
            params![doc_id, session_id, position],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((seq, edits_json, cursors_json)) = row else {
        return Ok(None);
    };

    let edits = edits_from_json(&edits_json)?;
    let cursors = cursors_from_json(&cursors_json)?;
    Ok(Some(Step {
        edits,
        cursors,
        new_pos: Seq(seq),
    }))
}

/// Commits the journal undo position to `pos` for `doc_id`, scoped to
/// `session_id`. Call ONLY after the corresponding buffer edit
/// (`rune_core::undo::apply_inverse`/`reapply`) has already succeeded.
/// The UPSERT creates this session's `session_documents` row on
/// its very first undo/redo for `doc_id` — no read-then-write split; `pos`
/// is always caller-supplied (from `undo_peek`/`redo_peek`), never derived
/// from a value this call itself reads.
pub fn move_undo_pos(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
    pos: Seq,
) -> Result<(), Error> {
    tx.execute(
        "INSERT INTO session_documents(session_id, doc_id, current_seq) VALUES(?1,?2,?3) \
         ON CONFLICT(session_id, doc_id) DO UPDATE SET current_seq=excluded.current_seq",
        params![session_id, doc_id, pos],
    )?;
    Ok(())
}

/// The effective journal position for `doc_id` as seen by `session_id`:
/// this session's own undo pointer if set, else `MAX(seq)` among only this
/// session's own events for `doc_id`, else 0 if this session has no events
/// for `doc_id` at all.
pub fn current_seq(
    tx: &Transaction<'_>,
    session_id: SessionId,
    doc_id: DocId,
) -> Result<Seq, Error> {
    let seq: i64 = tx.query_row(
        "SELECT COALESCE(
            (SELECT current_seq FROM session_documents WHERE session_id = ?1 AND doc_id = ?2),
            (SELECT MAX(seq) FROM events WHERE doc_id = ?2 AND session_id = ?1),
            0)",
        params![session_id, doc_id],
        |r| r.get(0),
    )?;
    Ok(Seq(seq))
}

/// `doc_id`'s own edit rows with seq in `(from_seq, to_seq]`, each tagged
/// with its seq, ordered ascending. Session-scoped: only ever this
/// session's own edits. Read-only, so it takes `&Connection` rather than
/// `&Transaction` — callable from either the writer's own transaction (via
/// `Transaction`'s `Deref<Target=Connection>` coercion) or a plain read
/// connection.
pub(crate) fn edits_in_range(
    conn: &rusqlite::Connection,
    session_id: SessionId,
    doc_id: DocId,
    from_seq: Seq,
    to_seq: Seq,
) -> Result<Vec<EditRow>, Error> {
    let mut stmt = conn.prepare(
        "SELECT seq, edits FROM events \
         WHERE doc_id=?1 AND session_id=?2 AND seq > ?3 AND seq <= ?4 ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![doc_id, session_id, from_seq, to_seq], |r| {
        Ok((r.get::<_, Seq>(0)?, r.get::<_, String>(1)?))
    })?;

    rows.map(|row| {
        let (seq, edits_json) = row?;
        let edits = edits_from_json(&edits_json)?;
        Ok(EditRow { seq, edits })
    })
    .collect::<Result<Vec<_>, Error>>()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::test_support::{insert_test_document, open};

    /// A corrupt edits payload must be returned as an error, never
    /// silently folded into "nothing to undo".
    #[test]
    fn corrupt_edits_payload_surfaces_as_error_not_ok_false() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("establish session");
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        tx.execute(
            "INSERT INTO events(doc_id, session_id, edits, cursors_before, cursors_after, at) \
             VALUES(?1, ?2, 'not valid json', '[]', '[]', '2026-01-01T00:00:00.000000000Z')",
            params![doc_id, session_id],
        )
        .expect("seed corrupt event");

        let err = undo_peek(&tx, session_id, doc_id).expect_err("must surface, not Ok(None)");
        assert!(matches!(err, Error::CorruptPayload(_)));
        tx.commit().expect("commit");
    }
}
