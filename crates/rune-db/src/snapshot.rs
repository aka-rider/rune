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
#[path = "snapshot_tests.rs"]
mod tests;
