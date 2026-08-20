//! Recovery-anchor snapshots and `recover_document`'s forward replay. A
//! snapshot is a PURE recovery anchor: it exists only to bound how far
//! `recover_document` ever has to replay, never a source-of-truth taxonomy
//! (that lives in `observations`, WP4). Every snapshot's content flows
//! through `blob::put_blob` before it's ever discarded from the journal's
//! live window (`journal::append_edit`'s future-truncation deletes both
//! `events` and `snapshots` rows, but the blob a surviving snapshot still
//! points to is untouched).

use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use rune_core::buffer::Buffer;
use rune_core::cursor::Cursor;
use rune_core::undo::reapply;

use crate::Error;
use crate::blob::{get_blob, put_blob};
use crate::ids::{DocId, Seq, SessionId};
use crate::journal::edits_in_range;
use crate::session::format_rfc3339_nanos;

/// Stores a recovery anchor for `doc_id` at journal position `seq`, tagged
/// with `session_id` — a snapshot anchors ONE session's own replay window;
/// two sessions editing the same `doc_id` keep entirely separate anchor
/// chains. `seq` should be the most recently returned seq from
/// `journal::append_edit` so `recover_document` can find this snapshot as
/// the closest anchor for any replay.
pub(crate) fn create_snapshot(
    tx: &Transaction<'_>,
    session_id: SessionId,
    now: SystemTime,
    doc_id: DocId,
    content: &str,
    seq: Seq,
) -> Result<i64, Error> {
    let hash = put_blob(tx, content.as_bytes())?;
    let at = format_rfc3339_nanos(now);
    tx.execute(
        "INSERT INTO snapshots(doc_id, session_id, blob_hash, seq, created_at) \
         VALUES(?1, ?2, ?3, ?4, ?5)",
        params![doc_id, session_id, hash, seq, at],
    )?;
    Ok(tx.last_insert_rowid())
}

/// A document reconstructed from the journal: the replayed content and the
/// caret state the editing session last journaled alongside it. An empty
/// `cursors` means the journal holds no caret for this reconstruction — the
/// caller keeps whatever caret it already had.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recovered {
    pub content: String,
    pub cursors: Vec<Cursor>,
}

/// Reconstructs `doc_id`'s content AS SEEN BY `session_id`:
///
/// 1. Read `session_id`'s own current undo position from
///    `session_documents` (no row, or NULL, means "at head" → use
///    `i64::MAX`).
/// 2. Find the newest snapshot tagged `session_id` with `seq <= target`,
///    tie-broken `seq DESC, id DESC` — separate call sites (`writer.rs`'s
///    periodic snapshot, `sync.rs`, `inherit.rs`, `load_anchor.rs`) can each
///    create a snapshot anchored at the SAME seq with no edit landing in
///    between, so `id DESC` picks the freshest row at that seq, never an
///    arbitrary tie; `anchor_content = ""` if none.
/// 3. Gather `session_id`'s own edit batches with `seq` in
///    `(anchor_seq, target]`.
/// 4. Forward-replay those batches onto `anchor_content`, one row at a
///    time, using `rune_core::undo::reapply`'s edit-application semantics
///    (ascending by `start` within a row, against a running buffer) —
///    replay surfaces an out-of-range edit as corruption instead of
///    silently clamping or skipping it. Every row here was produced by
///    this crate's own `append_edit`, so a replay failure means a
///    genuinely corrupt/inconsistent journal, which must be surfaced
///    rather than silently reconstructing wrong content.
///
/// Takes `&Connection` rather than `&Transaction` — this is a pure read, so
/// `rusqlite`'s `Transaction: Deref<Target=Connection>` lets writer-thread
/// callers (inside `retry::with_retry`'s `&Transaction`) and read-only
/// callers (`reader.rs`, a plain `&Connection`) share this ONE
/// implementation instead of two copies of the same query sequence.
pub fn recover_document(
    conn: &Connection,
    session_id: SessionId,
    doc_id: DocId,
) -> Result<Recovered, Error> {
    let current_seq: Option<Seq> = conn
        .query_row(
            "SELECT current_seq FROM session_documents WHERE session_id=?1 AND doc_id=?2",
            params![session_id, doc_id],
            |r| r.get::<_, Option<Seq>>(0),
        )
        .optional()?
        .flatten();
    let target = current_seq.unwrap_or(Seq(i64::MAX));

    let anchor: Option<(Seq, String)> = conn
        .query_row(
            "SELECT seq, blob_hash FROM snapshots \
             WHERE doc_id=?1 AND session_id=?2 AND seq <= ?3 \
             ORDER BY seq DESC, id DESC LIMIT 1",
            params![doc_id, session_id, target],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    let (anchor_seq, anchor_content) = match anchor {
        Some((seq, hash)) => {
            let bytes = get_blob(conn, &hash)?;
            // The anchor blob is always session-authored buffer content
            // (written by `create_snapshot` from a `&str`) re-entering the
            // String-typed edit buffer here — a decode failure means a
            // genuinely corrupt snapshot, which must surface as an error,
            // never be silently coerced (blob.rs module doc).
            let content = String::from_utf8(bytes).map_err(|e| {
                Error::CorruptPayload(crate::error::CorruptPayloadReason::NonUtf8Blob {
                    hash,
                    doc_id,
                    source: e,
                })
            })?;
            (seq, content)
        }
        None => (Seq(0), String::new()),
    };

    let rows = edits_in_range(conn, session_id, doc_id, anchor_seq, target)?;

    let mut buf = Buffer::new(anchor_content);
    let mut cursors = Vec::new();
    for row in rows {
        let seq = row.seq;
        buf = reapply(&buf, &row.edits).map_err(|e| {
            Error::ReplayFailed(Box::new(crate::error::ReplayFailure {
                doc_id,
                session_id,
                seq,
                source: e,
            }))
        })?;
        cursors = row.cursors_after;
    }
    Ok(Recovered {
        content: buf.content().to_string(),
        cursors,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::journal::{append_edit, move_undo_pos, undo_peek};
    use crate::journal_append::EditBatch;
    use crate::test_support::{insert_test_document, open};
    use rune_core::buffer::AppliedEdit;
    use rune_core::cursor::CursorId;
    use rune_core::undo::EditKind;

    fn cursor_at(offset: usize) -> Cursor {
        Cursor {
            position: offset,
            anchor: offset,
            desired_col: 0,
            id: CursorId::try_from(1).expect("test id is non-zero"),
        }
    }

    fn insert_test_session(conn: &Connection) -> SessionId {
        crate::session::establish_session(conn, SystemTime::now()).expect("establish session")
    }

    fn text_insert(s: &str) -> Vec<AppliedEdit> {
        vec![AppliedEdit {
            start: 0,
            end: 0,
            deleted: String::new(),
            insert: s.to_string(),
        }]
    }

    /// A corrupted journal row — two `AppliedEdit`s that collide on the
    /// identical post-edit `start` — must surface as `Error::ReplayFailed`
    /// rather than let `recover_document` silently pick a replay order.
    /// This is a shape `append_edit`'s own writer never produces (see
    /// `rune_core::undo::inverse_edits`'s coalescing), so it stands in for
    /// a row a build predating that guard could still have written.
    #[test]
    fn recover_surfaces_a_journal_row_with_colliding_applied_starts_as_replay_failed() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        let colliding = vec![
            AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "a".to_string(),
            },
            AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "b".to_string(),
            },
        ];
        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &colliding,
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append_edit");

        let err = recover_document(&tx, session_id, doc_id).expect_err("must refuse the replay");
        assert!(
            matches!(err, Error::ReplayFailed(_)),
            "expected ReplayFailed, got {err:?}"
        );
    }

    #[test]
    fn recover_with_no_snapshot_replays_from_empty() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("hello"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append_edit");

        let got = recover_document(&tx, session_id, doc_id).expect("recover");
        assert_eq!(got.content, "hello");
        tx.commit().expect("commit");
    }

    #[test]
    fn recover_uses_newest_anchor_at_or_before_target_and_replays_the_rest() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("file-edit-1"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append_edit 1");
        let seq1 = crate::journal::current_seq(&tx, session_id, doc_id).expect("seq1");
        create_snapshot(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            "file-edit-1",
            seq1,
        )
        .expect("snapshot");

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("-more"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append_edit 2");

        let got = recover_document(&tx, session_id, doc_id).expect("recover");
        assert_eq!(got.content, "-morefile-edit-1");
        tx.commit().expect("commit");
    }

    #[test]
    fn recover_respects_current_seq_after_undo() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("a"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append a");
        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("b"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append b");

        let step = undo_peek(&tx, session_id, doc_id)
            .expect("undo_peek")
            .expect("something to undo");
        move_undo_pos(&tx, session_id, doc_id, step.new_pos).expect("move_undo_pos");

        let got = recover_document(&tx, session_id, doc_id).expect("recover");
        assert_eq!(
            got.content, "a",
            "recovery must stop at the undone position"
        );
        tx.commit().expect("commit");
    }

    #[test]
    fn recover_restores_the_caret_the_last_replayed_row_journaled() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("hello"),
                cursors_before: &[cursor_at(0)],
                cursors_after: &[cursor_at(5)],
                kind: EditKind::Other,
            },
        )
        .expect("append hello");
        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("X"),
                cursors_before: &[cursor_at(5)],
                cursors_after: &[cursor_at(1)],
                kind: EditKind::Other,
            },
        )
        .expect("append X");

        let got = recover_document(&tx, session_id, doc_id).expect("recover");
        assert_eq!(got.content, "Xhello");
        assert_eq!(got.cursors, vec![cursor_at(1)]);
        tx.commit().expect("commit");
    }

    #[test]
    fn recover_stops_at_the_undone_positions_own_caret() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("a"),
                cursors_before: &[cursor_at(0)],
                cursors_after: &[cursor_at(1)],
                kind: EditKind::Other,
            },
        )
        .expect("append a");
        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("b"),
                cursors_before: &[cursor_at(1)],
                cursors_after: &[cursor_at(2)],
                kind: EditKind::Other,
            },
        )
        .expect("append b");

        let step = undo_peek(&tx, session_id, doc_id)
            .expect("undo_peek")
            .expect("something to undo");
        move_undo_pos(&tx, session_id, doc_id, step.new_pos).expect("move_undo_pos");

        let got = recover_document(&tx, session_id, doc_id).expect("recover");
        assert_eq!(got.content, "a");
        assert_eq!(got.cursors, vec![cursor_at(1)]);
        tx.commit().expect("commit");
    }

    /// A snapshot anchored at the journal head leaves nothing to replay, so
    /// there is no journaled caret to report — the caller keeps its own.
    #[test]
    fn an_empty_replay_range_reports_no_journaled_caret() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("anchored"),
                cursors_before: &[cursor_at(0)],
                cursors_after: &[cursor_at(8)],
                kind: EditKind::Other,
            },
        )
        .expect("append_edit");
        let seq = crate::journal::current_seq(&tx, session_id, doc_id).expect("seq");
        create_snapshot(&tx, session_id, SystemTime::now(), doc_id, "anchored", seq)
            .expect("snapshot");

        let got = recover_document(&tx, session_id, doc_id).expect("recover");
        assert_eq!(got.content, "anchored");
        assert!(got.cursors.is_empty());
        tx.commit().expect("commit");
    }

    #[test]
    fn an_empty_cursors_payload_reports_no_journaled_caret() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("no caret"),
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append_edit");

        let got = recover_document(&tx, session_id, doc_id).expect("recover");
        assert_eq!(got.content, "no caret");
        assert!(got.cursors.is_empty());
        tx.commit().expect("commit");
    }

    /// A row written before caret recovery existed can hold SQL NULL there.
    #[test]
    fn a_null_cursors_column_reports_no_journaled_caret() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            EditBatch {
                edits: &text_insert("legacy row"),
                cursors_before: &[cursor_at(0)],
                cursors_after: &[cursor_at(10)],
                kind: EditKind::Other,
            },
        )
        .expect("append_edit");
        tx.execute(
            "UPDATE events SET cursors_after = NULL WHERE doc_id=?1 AND session_id=?2",
            params![doc_id, session_id],
        )
        .expect("null the column");

        let got = recover_document(&tx, session_id, doc_id).expect("recover");
        assert_eq!(got.content, "legacy row");
        assert!(got.cursors.is_empty());
        tx.commit().expect("commit");
    }

    /// A row written before the `kind` column existed holds SQL NULL there
    /// — `recover_document`'s content/caret reconstruction must be
    /// unaffected, and reading the row back through `edits_in_range` must
    /// report exactly [`EditKind::Other`], the same "ungrouped" behavior
    /// every row had before this column existed.
    #[test]
    fn an_old_shape_row_with_no_kind_recovers_exactly_as_before() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        tx.execute(
            "INSERT INTO events(doc_id, session_id, edits, cursors_before, cursors_after, at) \
             VALUES(?1, ?2, ?3, '[]', '[]', '2026-01-01T00:00:00.000000000Z')",
            params![
                doc_id,
                session_id,
                crate::payload::edits_to_json(&text_insert("pre-existing")).expect("encode"),
            ],
        )
        .expect("seed a row written before the kind column existed");

        let got = recover_document(&tx, session_id, doc_id).expect("recover");
        assert_eq!(got.content, "pre-existing");

        let target = crate::journal::current_seq(&tx, session_id, doc_id).expect("current_seq");
        let rows =
            crate::journal::edits_in_range(&tx, session_id, doc_id, Seq(0), target).expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, EditKind::Other);
        tx.commit().expect("commit");
    }

    /// A live session that journals several edit batches under distinct
    /// [`EditKind`]s must recover, in a fresh session, the exact same
    /// ordered sequence of kinds it pushed — the data the undo-ladder
    /// grouping (`rune-tui`'s `undogroup`) walks to decide which runs of
    /// steps belong together.
    #[test]
    fn recovery_reconstructs_the_same_edit_kind_sequence_the_live_session_pushed() {
        let mut conn = open();
        let session_id = insert_test_session(&conn);
        let tx = conn.transaction().expect("tx");
        let doc_id = insert_test_document(&tx);

        let pushed = [
            EditKind::Insert,
            EditKind::Insert,
            EditKind::Insert,
            EditKind::DeleteLeft,
            EditKind::Paste,
            EditKind::Insert,
        ];
        for (i, kind) in pushed.iter().enumerate() {
            append_edit(
                &tx,
                session_id,
                SystemTime::now(),
                doc_id,
                EditBatch {
                    edits: &text_insert(&format!("s{i}")),
                    cursors_before: &[],
                    cursors_after: &[],
                    kind: *kind,
                },
            )
            .expect("append");
        }

        let target = crate::journal::current_seq(&tx, session_id, doc_id).expect("current_seq");
        let rows =
            crate::journal::edits_in_range(&tx, session_id, doc_id, Seq(0), target).expect("rows");
        let recovered_kinds: Vec<EditKind> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(recovered_kinds, pushed);
        tx.commit().expect("commit");
    }
}
