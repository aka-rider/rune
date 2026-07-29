//! `Load` — reads a path fresh from disk, resolves document identity,
//! records the sighting, and returns everything the caller needs to decide
//! how to display it. Ported from Go's `Load`. Every step below
//! either does `vfs` I/O with no transaction open, or opens its own short
//! `retry::with_retry` transaction (plan binding rule, Go invariant I1).
//!
//! The Adoption Contract (`observation.rs` module doc) governs `saved_obs`
//! here: first-ever load anchors a recovery snapshot and adopts it as-is;
//! a hash-equal reload heal-adopts (crash-between-swap-and-ack recovery); a
//! divergent reload records a bare, uncorrelated sighting and leaves
//! `saved_obs` untouched.

use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use rune_core::buffer::AppliedEdit;
use rune_vfs::Vfs;

use crate::Error;
use crate::adopt;
use crate::document::{self, DocRef};
use crate::observation::{self, ObsId};
use crate::retry;
use crate::sync::SyncState;

/// The outcome of [`load`]: the raw disk bytes, the journal-reconstructed
/// content (identical to `disk_content` when the document has no history),
/// and the [`SyncState`] that follows from recording this sighting. Port of
/// `observation.go:189-205` (`LoadResult`).
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
/// `session_id`. Port of `snapshot.go:240-260` (`HasHistory`).
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

/// The session_id attached to whichever row — across `doc_id`'s events and
/// snapshots together — carries the highest seq. Ties break by higher
/// `session_id`. `None` means `doc_id` has no session-scoped activity
/// recorded at all. Shared by [`find_inheritable_draft`] and
/// `reaper::session_is_reapable`. Port of `load.go:202-235`
/// (`mostRecentSessionForDoc`).
pub(crate) fn most_recent_session_for_doc(
    tx: &Transaction<'_>,
    doc_id: i64,
) -> Result<Option<i64>, Error> {
    tx.query_row(
        "SELECT session_id FROM ( \
            SELECT session_id, seq FROM events    WHERE doc_id=?1 \
            UNION ALL \
            SELECT session_id, seq FROM snapshots WHERE doc_id=?1 \
         ) \
         ORDER BY seq DESC, session_id DESC LIMIT 1",
        params![doc_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(Error::from)
}

fn is_session_alive(
    tx: &Transaction<'_>,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    session_id: i64,
) -> Result<bool, Error> {
    let row: Option<(i64, String)> = tx
        .query_row(
            "SELECT pid, proc_started_at FROM sessions WHERE id=?1",
            params![session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((pid, started_at)) = row else {
        return Ok(false); // the session row itself is gone -> unambiguously not alive
    };
    Ok(liveness_check(pid, &started_at))
}

/// Looks for a DIFFERENT, now-confirmed-dead session's unsaved content for
/// `doc_id`. Called ONLY from [`load`]'s `!has_history` branch, before this
/// session has written anything of its own. Returns `(draft, false)` for
/// disk content unchanged for every "nothing to inherit" case: no other
/// session ever touched this doc, the most recent one is still alive, disk
/// moved since that dead session's own last-known baseline, or its content
/// happens to hash-equal disk anyway. Port of `load.go:237-302`
/// (`findInheritableDraft`).
fn find_inheritable_draft(
    conn: &mut Connection,
    liveness_check: &dyn Fn(i64, &str) -> bool,
    doc_id: i64,
    disk_content: &str,
    disk_hash: &str,
) -> Result<(String, bool), Error> {
    let other_session_id = retry::with_retry(conn, |tx| most_recent_session_for_doc(tx, doc_id))?;
    let Some(other_session_id) = other_session_id else {
        return Ok((disk_content.to_string(), false));
    };

    let alive = retry::with_retry(conn, |tx| {
        is_session_alive(tx, liveness_check, other_session_id)
    })?;
    if alive {
        return Ok((disk_content.to_string(), false));
    }

    let other_baseline = retry::with_retry(conn, |tx| {
        observation::saved_obs_for(tx, other_session_id, doc_id)
    })?;
    if let Some(baseline) = &other_baseline
        && baseline.blob_hash != disk_hash
    {
        // Disk moved since the dead session's own last-known fact — not
        // safe to bridge (would silently discard the newer disk content).
        return Ok((disk_content.to_string(), false));
    }

    // Re-verify `other_session_id` is STILL the eligible candidate inside the
    // SAME transaction that reads its snapshot/events: a reap racing between
    // the checks above and here can delete `other_session_id`'s entire
    // footprint (`reaper::reap_session_footprint` clears `events`/
    // `snapshots`), and `recover_document` finding zero rows would
    // reconstruct "" — silently presenting an empty buffer for a document
    // with real content ([rune-db 3]). A raced reap must yield "not
    // eligible", never empty content.
    let recovered_draft = retry::with_retry(conn, |tx| {
        let still_candidate = most_recent_session_for_doc(tx, doc_id)?;
        if still_candidate != Some(other_session_id) {
            return Ok(None);
        }
        crate::snapshot::recover_document(tx, other_session_id, doc_id).map(Some)
    })?;
    let Some(recovered_draft) = recovered_draft else {
        return Ok((disk_content.to_string(), false));
    };
    if observation::hash_bytes(recovered_draft.as_bytes()) == disk_hash {
        return Ok((disk_content.to_string(), false)); // never actually diverged from disk
    }
    Ok((recovered_draft, true))
}

/// Reads `path` fresh from disk, resolves its document identity, records
/// the sighting, and returns everything the caller needs to decide how to
/// display it. Port of `load.go:38-200` (`Load`). `liveness_check` is
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
        // immediately find itself as "the most recent session".
        let (draft_content, inheriting) =
            find_inheritable_draft(conn, liveness_check, doc_id, &content, &hash)?;

        // First-ever load: the sighting IS the adoption — the recovery
        // anchor MUST commit first (the adoption asserts "the journal
        // reconstruction at load_seq equals this blob", which only holds
        // once the anchor snapshot exists). The anchor and the adoption
        // ALWAYS use disk content/hash, even when inheriting below.
        retry::with_retry(conn, |tx| {
            crate::snapshot::create_snapshot(tx, session_id, now, doc_id, &content, load_seq)
        })?;
        adopt::record_adoption(
            conn,
            doc_id,
            session_id,
            observation::ObservationMeta {
                blob_hash: &hash,
                seq: Some(load_seq),
                origin: "load",
            },
            &stat,
            now,
        )?;

        if inheriting {
            // Bridge the just-adopted disk content forward to the dead
            // session's draft via ONE synthetic replace-all edit, journaled
            // under THIS session's own session_id at the very next
            // position — exactly as if the user had just pasted the
            // recovered draft in.
            let bridge = vec![AppliedEdit {
                start: 0,
                end: content.len(),
                deleted: content.clone(),
                insert: draft_content.clone(),
            }];
            let seq = retry::with_retry(conn, |tx| {
                crate::journal::append_edit(tx, session_id, now, doc_id, &bridge, &[], &[])
            })?;
            bridge_seq = Some(seq);
            recovered = draft_content;
        }
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

    /// Dead-session inheritance must bail (fall back to plain disk content)
    /// when disk has moved on since the dead session's own last-known
    /// baseline — bridging stale content would silently discard the newer
    /// disk state (`load.go`'s `findInheritableDraft` review-fix doc
    /// comment). Exercises `find_inheritable_draft` directly: `Vfs`'s
    /// atomic-publish-only contract makes an inode-preserving "external
    /// overwrite" impossible to construct through the public `load` entry
    /// point (a real atomic swap legitimately mints a fresh inode, exactly
    /// like Go's own `OpenPath` — orphaning identity is a pre-existing,
    /// out-of-scope concern, not this test's target), so this seeds the
    /// document/session/observation state directly instead.
    #[test]
    fn dead_session_inheritance_bails_when_disk_hash_moved_on() {
        let mut conn = open();
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = conn.last_insert_rowid();

        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");

        // Session A loaded "session A's content" and typed "UNSAVED " on
        // top of it (unsaved edit), with its own baseline (saved_obs)
        // anchored on what it actually saw at load time.
        {
            let tx = conn.transaction().expect("tx");
            crate::snapshot::create_snapshot(
                &tx,
                session_a,
                SystemTime::now(),
                doc_id,
                "session A's content",
                0,
            )
            .expect("anchor snapshot");
            adopt::record_adoption_tx(
                &tx,
                doc_id,
                session_a,
                observation::ObservationMeta {
                    blob_hash: &observation::hash_bytes(b"session A's content"),
                    seq: Some(0),
                    origin: "load",
                },
                &observation::StatFacts {
                    mtime: "t".to_string(),
                    ..Default::default()
                },
                "t",
            )
            .expect("adopt load");
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

        // Disk has since moved on to content session A's own baseline never
        // saw ("disk moved on independently" != "session A's content").
        let disk_content = "disk moved on independently";
        let disk_hash = observation::hash_bytes(disk_content.as_bytes());

        let (draft, inheriting) =
            find_inheritable_draft(&mut conn, &always_dead, doc_id, disk_content, &disk_hash)
                .expect("find_inheritable_draft");

        assert!(!inheriting, "must bail, never bridge stale unsaved edits");
        assert_eq!(draft, disk_content, "must fall back to plain disk content");
    }

    /// The mirror-image control: when disk has NOT moved since the dead
    /// session's own baseline, its genuinely unsaved content IS inherited.
    #[test]
    fn dead_session_inheritance_bridges_when_disk_matches_dead_sessions_baseline() {
        let mut conn = open();
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/doc.md', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        let doc_id = conn.last_insert_rowid();
        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");

        let disk_content = "shared content";
        let disk_hash = observation::hash_bytes(disk_content.as_bytes());
        {
            let tx = conn.transaction().expect("tx");
            crate::snapshot::create_snapshot(
                &tx,
                session_a,
                SystemTime::now(),
                doc_id,
                disk_content,
                0,
            )
            .expect("anchor snapshot");
            adopt::record_adoption_tx(
                &tx,
                doc_id,
                session_a,
                observation::ObservationMeta {
                    blob_hash: &disk_hash,
                    seq: Some(0),
                    origin: "load",
                },
                &observation::StatFacts {
                    mtime: "t".to_string(),
                    ..Default::default()
                },
                "t",
            )
            .expect("adopt load");
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

        let (draft, inheriting) =
            find_inheritable_draft(&mut conn, &always_dead, doc_id, disk_content, &disk_hash)
                .expect("find_inheritable_draft");
        assert!(
            inheriting,
            "disk unchanged since the dead session's baseline -> safe to bridge"
        );
        assert_eq!(draft, "UNSAVED shared content");
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
}
