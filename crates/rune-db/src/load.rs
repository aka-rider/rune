//! `Load` — reads a path fresh from disk, resolves document identity,
//! records the sighting, and returns everything the caller needs to decide
//! how to display it. Every step below
//! either does `vfs` I/O with no transaction open, or opens its own short
//! `retry::with_retry` transaction (plan binding rule, invariant I1).
//!
//! The Adoption Contract (`observation.rs` module doc) governs `saved_obs`
//! here: first-ever load anchors a recovery snapshot and adopts it as-is;
//! a hash-equal reload heal-adopts (crash-between-swap-and-ack recovery); a
//! divergent reload records a bare, uncorrelated sighting and leaves
//! `saved_obs` untouched.

use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, Transaction, params};

use rune_vfs::Vfs;

use crate::Error;
use crate::adopt;
use crate::document::{self, DocRef};
use crate::inherit::find_inheritable_draft;
use crate::load_anchor::{LoadContext, anchor_first_load};
use crate::observation::{self, ObsId};
use crate::retry;
use crate::sync::SyncState;

/// The outcome of [`load`]: the raw disk bytes, the journal-reconstructed
/// content (identical to `disk_content` when the document has no history),
/// and the [`SyncState`] that follows from recording this sighting.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadResult {
    pub doc_id: i64,
    pub renamed_from: Option<String>,
    pub disk_content: String,
    pub recovered: String,
    pub has_history: bool,
    pub sync: SyncState,
    /// Hard-link count observed at load (0 if stat failed or the platform
    /// doesn't expose it) — `nlink > 1` means saving through this path
    /// forks the document from its other names on disk.
    pub nlink: i64,
    /// This session's CAS baseline for `doc_id` (`session_documents.
    /// saved_obs`) once `load` returns — the `expect` `Store::materialize`
    /// (WP5) needs for this session's very first save. `None` only if
    /// adoption somehow never happened for this session/doc pair (should
    /// not occur — every branch of `load` above either adopts or leaves a
    /// PRIOR adoption of this session's own in place — kept `Option` rather
    /// than assumed, matching this crate's "Options for absent facts" rule
    /// rather than a caller-visible panic risk).
    pub saved_obs: Option<ObsId>,
    /// The durable journal seq THIS session's own cross-session-inheritance
    /// bridge edit (`find_inheritable_draft`'s synthetic replace-all,
    /// journaled under `session_id`) committed to, when `load` performed
    /// one — `None` when it didn't (no inheritance, or `has_history` was
    /// already true). This is this session's own durable journal HEAD
    /// immediately after `load` returns: a fresh session has journaled
    /// nothing else of its own before or during `load`, so a caller seeding
    /// its local "last acked durable seq" tracking (`AppDb::last_known_seq`)
    /// MUST start from this, not an assumed `0` — an `undo`/`redo`
    /// committing `move_undo_pos` before its first ordinary `AppendEdit` ack
    /// lands would otherwise silently regress this session's own
    /// `current_seq` past a bridge edit that already advanced it.
    pub bridge_seq: Option<i64>,
}

/// Reports whether `doc_id` has any events or snapshots RECORDED BY
/// `session_id`.
pub fn has_history(tx: &Transaction<'_>, session_id: i64, doc_id: i64) -> Result<bool, Error> {
    tx.query_row(
        "SELECT EXISTS( \
            SELECT 1 FROM events WHERE doc_id=?1 AND session_id=?2 \
            UNION ALL \
            SELECT 1 FROM snapshots WHERE doc_id=?1 AND session_id=?2 \
         )",
        params![doc_id, session_id],
        |r| r.get(0),
    )
    .map_err(Error::from)
}

/// Reads `path` fresh from disk, resolves its document identity, records
/// the sighting, and returns everything the caller needs to decide how to
/// display it. `liveness_check` is
/// threaded in per-call (this `Store`'s injected liveness function) rather
/// than read from shared state — `OpKind::Load` carries it.
pub fn load(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: i64,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    path: &Path,
    now: SystemTime,
) -> Result<LoadResult, Error> {
    let resolved = vfs.resolve(path).map_err(Error::Io)?;
    let data = vfs.read(&resolved).map_err(Error::Io)?;

    let doc_ref: DocRef = document::open_path(conn, vfs, &resolved, now)?;
    let doc_id = doc_ref.id;

    // Hashing and blob storage operate on the raw disk bytes UNCONDITIONALLY
    // — a load must still durably record what's actually on disk even if it
    // turns out not to be valid UTF-8 (blob.rs's module doc: disk-sourced
    // content is never gated on decode success). Only the `content: String`
    // this function ultimately returns for the edit buffer requires the
    // file to be genuinely valid text — that check happens below, AFTER
    // this sighting is already durably recorded.
    let hash = retry::with_retry(conn, |tx| crate::blob::put_blob(tx, &data))?;

    // The journal position AT LOAD TIME.
    let load_seq = retry::with_retry(conn, |tx| {
        crate::journal::current_seq(tx, session_id, doc_id)
    })?;

    let stat = observation::stat_identity(vfs, &resolved);

    // BEFORE recording anything below — must reflect GENUINE prior history,
    // not history this very call is about to create.
    let has_hist = retry::with_retry(conn, |tx| has_history(tx, session_id, doc_id))?;

    // Content re-enters the String-typed edit buffer HERE: this is
    // session-editable content, which must be valid UTF-8 (rune-core's
    // `Buffer` has no other representation) — a load of genuinely non-text
    // content is a real, surfaced error, never silently mangled or dropped.
    let content = String::from_utf8(data)
        .map_err(|e| Error::Invalid(format!("load {}: non-utf8 content: {e}", path.display())))?;

    let mut recovered = content.clone();
    let mut bridge_seq = None;
    if has_hist {
        recovered = retry::with_retry(conn, |tx| {
            crate::snapshot::recover_document(tx, session_id, doc_id)
        })?;
    }

    if !has_hist {
        // Cross-session crash recovery (v10, B2/R2) — MUST run before this
        // session writes anything of its own for doc_id, else it would
        // immediately find itself as "the most recent session". First-ever
        // load: the sighting IS the adoption — the recovery anchor MUST
        // commit before the adoption (which asserts "the journal
        // reconstruction at load_seq equals this blob"). The anchor and the
        // adoption use disk content/hash for `Inherited::Disk`/`Bridged`;
        // `Inherited::Diverged` instead re-anchors on the dead session's own
        // baseline (`load_anchor`'s own doc comment) so the newer disk
        // content is never silently discarded.
        let inherited = find_inheritable_draft(conn, liveness_check, doc_id, &hash)?;
        let ctx = LoadContext {
            session_id,
            doc_id,
            load_seq,
            disk_hash: &hash,
            live_stat: &stat,
            now,
        };
        let outcome = anchor_first_load(conn, &ctx, &content, inherited)?;
        recovered = outcome.recovered;
        bridge_seq = outcome.bridge_seq;
    } else if hash == observation::hash_bytes(recovered.as_bytes()) {
        // Reload, hash-equality: heal-adopt only when there is something to
        // heal (an ordinary clean tab switch also lands here).
        let cur = retry::with_retry(conn, |tx| {
            observation::saved_obs_for(tx, session_id, doc_id)
        })?;
        let needs_heal = match &cur {
            None => true,
            Some(c) => c.blob_hash != hash,
        };
        if needs_heal {
            adopt::record_adoption(
                conn,
                doc_id,
                session_id,
                observation::ObservationMeta {
                    blob_hash: &hash,
                    seq: Some(load_seq),
                    origin: "resolve",
                },
                &stat,
                now,
            )?;
        }
    } else {
        // Reload, hashes differ: a bare, uncorrelated sighting — saved_obs
        // stays exactly where it was.
        let at = crate::session::format_rfc3339_nanos(now);
        retry::with_retry(conn, |tx| {
            observation::record_observation(
                tx,
                doc_id,
                session_id,
                observation::ObservationMeta {
                    blob_hash: &hash,
                    seq: None,
                    origin: "load",
                },
                &stat,
                &at,
            )
        })?;
    }

    let sync_state = retry::with_retry(conn, |tx| crate::sync::sync(tx, session_id, doc_id))?;
    let saved_obs = retry::with_retry(conn, |tx| {
        observation::saved_obs_for(tx, session_id, doc_id)
    })?
    .map(|o| o.id);

    Ok(LoadResult {
        doc_id,
        renamed_from: doc_ref.renamed_from,
        disk_content: content,
        recovered,
        has_history: has_hist,
        sync: sync_state,
        nlink: stat.nlink.unwrap_or(0),
        saved_obs,
        bridge_seq,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_core::buffer::AppliedEdit;
    use rune_vfs::Mem;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
        let temp = vfs.write_durable(path, bytes).expect("write_durable");
        vfs.rename_excl(&temp, path).expect("publish");
    }

    fn always_alive(_pid: i64, _started_at: &str) -> bool {
        true
    }

    fn always_dead(_pid: i64, _started_at: &str) -> bool {
        false
    }

    #[test]
    fn first_load_anchors_a_snapshot_and_adopts() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"hello world");

        let result = load(
            &mut conn,
            &vfs,
            session_id,
            &always_alive,
            path,
            SystemTime::now(),
        )
        .expect("load");
        assert_eq!(result.disk_content, "hello world");
        assert_eq!(result.recovered, "hello world");
        assert!(
            !result.has_history,
            "HasHistory must reflect PRIOR history only"
        );
        assert_eq!(result.sync.kind, crate::sync::SyncKind::Clean);

        let saved_obs_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_documents WHERE session_id=?1 AND doc_id=?2)",
                params![session_id, result.doc_id],
                |r| r.get(0),
            )
            .expect("check");
        assert!(saved_obs_exists, "first load must adopt");
    }

    /// End-to-end through the full [`load`] entry point (finding 8): a
    /// fresh session inheriting a dead session's draft must report the
    /// bridge edit's own durable seq in `LoadResult::bridge_seq`, exactly
    /// matching this session's own `current_seq` immediately after —
    /// a caller seeding `last_known_seq` from a hardcoded `0` instead would
    /// silently regress behind a `move_undo_pos`/`materialize` issued
    /// before this session's first ordinary `AppendEdit` ack lands.
    #[test]
    fn load_through_inheritance_reports_the_bridge_edits_own_durable_seq() {
        let mut conn = open();
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"shared content");

        // Session A loads, types an unsaved edit, then "dies" without
        // saving.
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
        let doc_id = load(
            &mut conn,
            &vfs,
            session_a,
            &always_alive,
            path,
            SystemTime::now(),
        )
        .expect("session a load")
        .doc_id;
        {
            let tx = conn.transaction().expect("tx");
            crate::journal::append_edit(
                &tx,
                session_a,
                SystemTime::now(),
                doc_id,
                &[AppliedEdit {
                    start: 0,
                    end: 0,
                    deleted: String::new(),
                    insert: "UNSAVED ".to_string(),
                }],
                &[],
                &[],
            )
            .expect("append_edit");
            tx.commit().expect("commit");
        }

        // Session B (a fresh process/session) loads the SAME doc after A
        // died — disk hasn't moved since A's own baseline, so B inherits
        // A's unsaved content via a bridge edit.
        let session_b =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
        let result = load(
            &mut conn,
            &vfs,
            session_b,
            &always_dead,
            path,
            SystemTime::now(),
        )
        .expect("session b load");

        assert_eq!(result.recovered, "UNSAVED shared content");
        let bridge_seq = result
            .bridge_seq
            .expect("a bridge edit must have been journaled for session b");

        let head = retry::with_retry(&mut conn, |tx| {
            crate::journal::current_seq(tx, session_b, doc_id)
        })
        .expect("current_seq");
        assert_eq!(
            head, bridge_seq,
            "bridge_seq must equal this session's own durable journal head"
        );
    }

    /// The mirror-image control: no cross-session inheritance happened (a
    /// document's very first-ever load, no prior session at all), so
    /// `bridge_seq` must be `None` — never a stale or fabricated seq.
    #[test]
    fn load_without_inheritance_reports_no_bridge_seq() {
        let mut conn = open();
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"hello world");
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");

        let result = load(
            &mut conn,
            &vfs,
            session_id,
            &always_alive,
            path,
            SystemTime::now(),
        )
        .expect("load");
        assert_eq!(result.bridge_seq, None);
    }

    /// End-to-end DATA-LOSS regression through the full public [`load`]
    /// entry point: session A opens (H0), edits (journaled durably, never
    /// saved), the file is overwritten by an external ATOMIC SWAP (H1,
    /// mints a new inode — `document::open_path_by_inode`'s reclaim branch,
    /// B3), and session A dies without saving. Session B, a fresh process
    /// reopening the same path, must re-anchor on A's own baseline (H0),
    /// bridge H0 -> A's draft, and end up `Diverged` against disk's current
    /// content (H1) — never silently dropping A's draft in favor of
    /// whatever is on disk now.
    #[test]
    fn diverged_load_bridges_the_dead_sessions_own_baseline_not_disk() {
        use rune_vfs::Vfs;

        let mut conn = open();
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"session A's content");

        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
        let doc_id = load(
            &mut conn,
            &vfs,
            session_a,
            &always_alive,
            path,
            SystemTime::now(),
        )
        .expect("session a load")
        .doc_id;
        {
            let tx = conn.transaction().expect("tx");
            crate::journal::append_edit(
                &tx,
                session_a,
                SystemTime::now(),
                doc_id,
                &[AppliedEdit {
                    start: 0,
                    end: 0,
                    deleted: String::new(),
                    insert: "UNSAVED ".to_string(),
                }],
                &[],
                &[],
            )
            .expect("append_edit");
            tx.commit().expect("commit");
        }

        // An external atomic-swap overwrite — same path, a NEW inode.
        vfs.save_atomic(path, b"disk moved on independently")
            .expect("external atomic swap");

        let session_b =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
        let result = load(
            &mut conn,
            &vfs,
            session_b,
            &always_dead,
            path,
            SystemTime::now(),
        )
        .expect("session b load");

        assert_eq!(
            result.doc_id, doc_id,
            "the swap must reuse A's document row"
        );
        assert_eq!(
            result.recovered, "UNSAVED session A's content",
            "must bridge from A's own baseline, never silently drop A's draft"
        );
        assert_eq!(result.sync.kind, crate::sync::SyncKind::Diverged);

        let bridge_seq = result
            .bridge_seq
            .expect("a bridge edit must have been journaled for session b");
        let head = retry::with_retry(&mut conn, |tx| {
            crate::journal::current_seq(tx, session_b, doc_id)
        })
        .expect("current_seq");
        assert_eq!(head, bridge_seq);

        let saved_obs = retry::with_retry(&mut conn, |tx| {
            observation::saved_obs_for(tx, session_b, doc_id)
        })
        .expect("saved_obs_for")
        .expect("session b adopted a baseline");
        assert_eq!(
            saved_obs.blob_hash,
            observation::hash_bytes(b"session A's content"),
            "saved_obs (CAS baseline) must be A's own H0, not disk's H1"
        );
    }
}
