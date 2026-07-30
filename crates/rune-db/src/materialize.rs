//! `Materialize` — the CAS write protocol that turns a buffer into the
//! user's destination file. WP7 inverted this module's shape: it used to
//! run the ENTIRE protocol — including the `vfs.write_durable`/`exchange`
//! disk publish itself — as one op on the writer thread's single FIFO, so a
//! dead writer thread made saving impossible even though the publish itself
//! needs nothing from the database ([rune-db 1]). The disk publish now runs
//! on the CALLER's own thread (`rune-tui`'s save `Cmd`, via its OWN `Vfs`
//! handle), and this module is bookkeeping-only around it:
//!
//! - [`prepare_materialize`] — pure DB read, no `vfs` call at all: hands the
//!   caller the CAS baseline (`expect`'s hash) and the bound path to check
//!   its own target against, before it does any disk I/O.
//! - The caller performs the actual `resolve`/read/hash-compare/
//!   `write_durable`/`exchange`(or `rename_excl`)/read-displaced dance
//!   itself, using [`rune_vfs::published_not_durable`] to tell "the swap
//!   already took effect" apart from "it never happened" the same way
//!   `Vfs::save_atomic` does.
//! - [`record_materialize_outcome`] — records what the caller's vfs work
//!   concluded (a conflict, a plain commit, or a swap-race commit) as the
//!   same CAS bookkeeping `commit_save`/`record_fresh` always did, now fed
//!   caller-supplied bytes/stat facts instead of calling `vfs` itself.
//!
//! A dead writer thread can still fail [`prepare_materialize`] or
//! [`record_materialize_outcome`]'s enqueue (`Error::WriterGone`) — but by
//! then the disk publish is either not yet attempted (prepare failed: the
//! caller falls back to an uncoordinated direct write, same as a document
//! with no store binding at all) or already physically complete (record
//! failed: the user's bytes are safely on disk; only this session's CAS
//! bookkeeping is lost, which degrades the store, not the save). Every
//! `vfs` call this module used to make is gone; the sibling relation
//! `lib.rs` documents is now structural, not just a convention two modules
//! happen to follow.

use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, params};

use crate::Error;
use crate::observation::{self, ObsId, Observation, ObservationMeta, StatFacts};
use crate::rebind::{Rebind, rebind_document_tx};
use crate::retry;

pub use crate::materialize_types::{DocSession, MatResult, MaterializeOutcome, MaterializePrep};

/// Step 1 (now caller-facing): fetches the CAS decision data for a
/// `!bind_new` materialize attempt — the bound path to check the caller's
/// own target against, and the baseline hash to CAS-compare the live target
/// against. Pure DB read, no `vfs` call ([rune-db 1]'s fix: this can run
/// even while every disk in the workspace is unreachable, and the caller's
/// OWN subsequent disk work never depends on the writer thread being alive
/// a moment longer than it takes to answer this one query).
pub fn prepare_materialize(
    conn: &mut Connection,
    doc_id: i64,
    expect: ObsId,
    bind_new: bool,
) -> Result<MaterializePrep, Error> {
    if bind_new {
        return Ok(MaterializePrep::default());
    }

    let db_path: String = retry::with_retry(conn, |tx| {
        tx.query_row(
            "SELECT path FROM documents WHERE id=?1",
            params![doc_id],
            |r| r.get(0),
        )
        .map_err(Error::from)
    })?;
    if db_path.is_empty() {
        return Err(Error::Invalid(format!(
            "materialize doc {doc_id}: no path bound (untitled document)"
        )));
    }
    let expect_obs = retry::with_retry(conn, |tx| observation::get_observation(tx, expect))?;
    Ok(MaterializePrep {
        bound_path: Some(db_path),
        expect_hash: expect_obs.blob_hash,
    })
}

/// Steps 4-5 (now caller-facing): records what the caller's own `vfs` work
/// concluded — a CAS conflict, a plain commit, or a swap-race commit — as
/// the same blob+observation(+rebind) bookkeeping `commit_save`/
/// `record_fresh` always did, fed caller-supplied bytes/stat facts instead
/// of calling `vfs`. `resolved_path`/`seq` are the caller's own
/// enqueue-time-captured facts (§1.4.2/§1.4.8), never re-derived here.
/// `resolved_path` is the caller's own already-`vfs.resolve`d destination —
/// converted to the checked `TEXT`-column string here (A4, [rune-db 6]: a
/// non-UTF-8 path is rejected loudly rather than mangled), the one place
/// this module still needs a `Path` at all, and it never touches disk to
/// produce it.
pub fn record_materialize_outcome(
    conn: &mut Connection,
    ds: DocSession,
    resolved_path: &Path,
    seq: i64,
    now: SystemTime,
    outcome: MaterializeOutcome,
) -> Result<MatResult, Error> {
    match outcome {
        MaterializeOutcome::Conflict { data, origin, stat } => {
            let fresh = record_fresh_from_stat(conn, ds, &data, origin, &stat, now)?;
            Ok(MatResult {
                fresh: Some(fresh),
                ..Default::default()
            })
        }
        MaterializeOutcome::Committed { data, stat } => {
            let resolved_str = crate::paths::to_db_string(resolved_path)?;
            let facts = CommitFacts {
                resolved_path: &resolved_str,
                data: &data,
                seq,
                stat: &stat,
            };
            let saved = commit_save_from_stat(conn, ds, facts, now)?;
            Ok(MatResult {
                committed: true,
                saved: Some(saved),
                ..Default::default()
            })
        }
        MaterializeOutcome::Raced {
            data,
            stat,
            displaced,
            displaced_stat,
        } => {
            let fresh = record_fresh_from_stat(conn, ds, &displaced, "swap", &displaced_stat, now)?;
            let resolved_str = crate::paths::to_db_string(resolved_path)?;
            let facts = CommitFacts {
                resolved_path: &resolved_str,
                data: &data,
                seq,
                stat: &stat,
            };
            let saved = commit_save_from_stat(conn, ds, facts, now)?;
            Ok(MatResult {
                committed: true,
                raced: true,
                saved: Some(saved),
                fresh: Some(fresh),
                missing: false,
            })
        }
    }
}

/// Puts `data`'s raw bytes as a blob and records an observation of them at
/// caller-supplied `stat`, for the `Conflict{Fresh}` outcomes. `data` is
/// disk-sourced — the target's live content on a CAS refusal, or a racer's
/// displaced bytes on a swap-race (§1.4.10 mandates this capture happens
/// unconditionally, never gated on UTF-8 validity: see `blob.rs`'s module
/// doc). The blob put and its referencing observation insert commit as ONE
/// transaction (`observe_from_stat_tx`) — never two, closing the
/// cross-process GC race [rune-db 2]. No `vfs` call: `stat` is the
/// caller's own fact, gathered on the thread that did the actual disk work.
pub(crate) fn record_fresh_from_stat(
    conn: &mut Connection,
    ds: DocSession,
    data: &[u8],
    origin: &str,
    stat: &StatFacts,
    now: SystemTime,
) -> Result<Observation, Error> {
    let at = crate::session::format_rfc3339_nanos(now);
    retry::with_retry(conn, |tx| {
        observation::observe_from_stat_tx(
            tx,
            ds.session_id,
            ds.doc_id,
            stat,
            &at,
            observation::ObserveInput {
                data,
                seq: None,
                origin,
            },
        )
    })
}

/// The bytes/path/seq/stat a committed write is recorded against — bundled
/// for the same argument-count reason as [`DocSession`].
#[derive(Clone, Copy, Debug)]
struct CommitFacts<'a> {
    /// The destination path, already resolved+stringified by the caller.
    resolved_path: &'a str,
    /// The bytes actually written (used to `put_blob` under the hash of
    /// what's now physically on disk).
    data: &'a [u8],
    /// The caller's save-start-captured journal position — NEVER re-read
    /// here.
    seq: i64,
    /// The destination's post-publish stat, gathered by the caller.
    stat: &'a StatFacts,
}

/// ONE tx — blob put (hash of the bytes actually written) + observation
/// (`origin='save'`) + `saved_obs` update + re-Bind (path/inode/device/
/// `kind='file'`, caller-supplied post-swap stat). No `vfs` call: every
/// disk-sourced fact `facts` carries was already gathered by the caller on
/// its own thread, before this op was ever enqueued (I1's "no DB tx across
/// a vfs call" contract, now trivially true — this function makes no vfs
/// call at all). The blob put and the observation that references its hash
/// used to be two separate transactions — a cross-process GC sweep landing
/// between them could delete the blob before the reference committed,
/// failing the reference with no retry ([rune-db 2]); both commit
/// atomically here, following the pattern `snapshot::create_snapshot`
/// already uses.
fn commit_save_from_stat(
    conn: &mut Connection,
    ds: DocSession,
    facts: CommitFacts<'_>,
    now: SystemTime,
) -> Result<Observation, Error> {
    let at = crate::session::format_rfc3339_nanos(now);
    let resolved_str = facts.resolved_path.to_string();

    retry::with_retry(conn, |tx| {
        let hash = crate::blob::put_blob(tx, facts.data)?;

        let obs = crate::adopt::record_adoption_tx(
            tx,
            ds.doc_id,
            ds.session_id,
            ObservationMeta {
                blob_hash: &hash,
                seq: Some(facts.seq),
                origin: "save",
            },
            facts.stat,
            &at,
        )?;

        rebind_document_tx(
            tx,
            ds.doc_id,
            Rebind {
                path: &resolved_str,
                stat: facts.stat,
                at: &at,
            },
        )?;

        Ok(obs)
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::path::Path;

    use rune_vfs::{Mem, Vfs};

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    fn seed_doc_with_path(conn: &Connection, path: &str) -> i64 {
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES (?1, 'x', 'x')",
            params![path],
        )
        .expect("seed doc");
        conn.last_insert_rowid()
    }

    fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
        let temp = vfs.write_durable(path, bytes).expect("write_durable");
        vfs.rename_excl(&temp, path).expect("publish");
    }

    /// Seeds an `observations` row whose CAS baseline is the hash of
    /// `content` — `blob_hash` is FK-constrained to `blobs.hash`, so
    /// `content` is durably stored first via `put_blob` (never a
    /// hand-picked hash string with no backing row).
    fn record_obs(conn: &Connection, doc_id: i64, session_id: i64, content: &str) -> ObsId {
        let hash = crate::blob::put_blob(conn, content.as_bytes()).expect("seed blob");
        conn.execute(
            "INSERT INTO observations(doc_id, session_id, blob_hash, seq, size, mtime, origin, at) \
             VALUES(?1,?2,?3,NULL,0,'t','load','t')",
            params![doc_id, session_id, hash],
        )
        .expect("seed observation");
        conn.last_insert_rowid()
    }

    fn stat_of(vfs: &Mem, path: &Path) -> StatFacts {
        observation::stat_identity(vfs, path)
    }

    /// [`prepare_materialize`] is pure DB bookkeeping — it must never touch
    /// `vfs` at all. Proven by never constructing a `Vfs` in this test:
    /// there is nothing for it to call even if it tried.
    #[test]
    fn prepare_materialize_is_vfs_free_and_returns_the_bound_path_and_expect_hash() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc_with_path(&conn, "/doc.md");
        let expect = record_obs(&conn, doc_id, session_id, "original");

        let prep = prepare_materialize(&mut conn, doc_id, expect, false).expect("prepare");
        assert_eq!(prep.bound_path.as_deref(), Some("/doc.md"));
        assert_eq!(prep.expect_hash, observation::hash_bytes(b"original"));
    }

    /// `bind_new=true` skips the bound-path/CAS-baseline lookup entirely —
    /// `materialize_create`'s original shape never consulted `expect`.
    #[test]
    fn prepare_materialize_bind_new_is_a_pure_default_no_query() {
        let mut conn = open();
        // No document row at all — if `prepare_materialize` tried to read
        // one for `bind_new`, this would error instead of returning
        // cleanly.
        let prep = prepare_materialize(&mut conn, 999, 0, true).expect("prepare bind_new");
        assert_eq!(prep, MaterializePrep::default());
    }

    /// An untitled document (empty bound path) must refuse rather than
    /// silently proceeding — the caller has nothing to CAS its target
    /// against.
    #[test]
    fn prepare_materialize_refuses_an_untitled_document() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc_with_path(&conn, "");
        let expect = record_obs(&conn, doc_id, session_id, "irrelevant");

        let err = prepare_materialize(&mut conn, doc_id, expect, false)
            .expect_err("untitled document must refuse");
        assert!(matches!(err, Error::Invalid(_)));
    }

    /// A CAS conflict (the caller's own vfs read/hash-compare found the
    /// live target disagreeing with `expect`) records the live bytes as a
    /// fresh, `origin='probe'` observation and never marks the write
    /// committed — using ONLY caller-supplied bytes/stat, no `vfs` call.
    #[test]
    fn record_materialize_outcome_conflict_records_fresh_and_never_commits() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"external content");
        let doc_id = seed_doc_with_path(&conn, "/doc.md");
        let stat = stat_of(&vfs, path);

        let result = record_materialize_outcome(
            &mut conn,
            DocSession { doc_id, session_id },
            Path::new("/doc.md"),
            1,
            SystemTime::now(),
            MaterializeOutcome::Conflict {
                data: b"external content".to_vec(),
                origin: "probe",
                stat,
            },
        )
        .expect("record conflict");

        assert!(!result.committed);
        let fresh = result.fresh.expect("fresh observation recorded");
        assert_eq!(fresh.origin, "probe");
        assert_eq!(
            fresh.blob_hash,
            observation::hash_bytes(b"external content")
        );
    }

    /// A plain committed write records the blob/observation and rebinds the
    /// document's row to the resolved path — the same effect `commit_save`
    /// always had, now driven by caller-supplied facts only.
    #[test]
    fn record_materialize_outcome_committed_records_save_and_rebinds() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"original");
        let doc_id = seed_doc_with_path(&conn, "/doc.md");
        let stat = stat_of(&vfs, path);

        let result = record_materialize_outcome(
            &mut conn,
            DocSession { doc_id, session_id },
            Path::new("/doc.md"),
            7,
            SystemTime::now(),
            MaterializeOutcome::Committed {
                data: b"new content".to_vec(),
                stat,
            },
        )
        .expect("record committed");

        assert!(result.committed);
        let saved = result.saved.expect("saved observation recorded");
        assert_eq!(saved.origin, "save");
        assert_eq!(saved.seq, Some(7));
        assert_eq!(saved.blob_hash, observation::hash_bytes(b"new content"));

        let bound_path: String = conn
            .query_row(
                "SELECT path FROM documents WHERE id=?1",
                params![doc_id],
                |r| r.get(0),
            )
            .expect("read back bound path");
        assert_eq!(bound_path, "/doc.md");
    }

    /// A swap-race outcome records BOTH the displaced bytes (`origin=
    /// 'swap'`) and our own committed write — §1.4.10's unconditional
    /// displaced-bytes capture, driven entirely by caller-supplied facts.
    #[test]
    fn record_materialize_outcome_raced_records_both_displaced_and_committed() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"our content");
        let doc_id = seed_doc_with_path(&conn, "/doc.md");
        let stat = stat_of(&vfs, path);

        let result = record_materialize_outcome(
            &mut conn,
            DocSession { doc_id, session_id },
            Path::new("/doc.md"),
            3,
            SystemTime::now(),
            MaterializeOutcome::Raced {
                data: b"our content".to_vec(),
                stat,
                displaced: b"racer bytes".to_vec(),
                displaced_stat: StatFacts::default(),
            },
        )
        .expect("record raced");

        assert!(result.committed);
        assert!(result.raced);
        let fresh = result.fresh.expect("displaced bytes recorded");
        assert_eq!(fresh.origin, "swap");
        assert_eq!(fresh.blob_hash, observation::hash_bytes(b"racer bytes"));
        let saved = result.saved.expect("our write recorded");
        assert_eq!(saved.blob_hash, observation::hash_bytes(b"our content"));

        let blob = retry::with_retry(&mut conn, |tx| crate::blob::get_blob(tx, &fresh.blob_hash))
            .expect("racer bytes durably stored as a blob");
        assert_eq!(blob, b"racer bytes");
    }

    /// §1.4.10 mandates capturing displaced bytes unconditionally — even
    /// when they are NOT valid UTF-8 (a binary file, or another process's
    /// own in-progress non-text write landing in the swap window).
    #[test]
    fn record_materialize_outcome_conflict_with_non_utf8_bytes_captures_them_byte_exact() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc_with_path(&conn, "/doc.md");
        let racer_bytes: &[u8] = &[0xff, 0xfe, 0x00, 0x9f, 0x92, 0x96, 0x80];

        let result = record_materialize_outcome(
            &mut conn,
            DocSession { doc_id, session_id },
            Path::new("/doc.md"),
            1,
            SystemTime::now(),
            MaterializeOutcome::Conflict {
                data: racer_bytes.to_vec(),
                origin: "swap",
                stat: StatFacts::default(),
            },
        )
        .expect("record conflict with non-utf8 bytes");

        let fresh = result.fresh.expect("fresh observation recorded");
        let blob = retry::with_retry(&mut conn, |tx| crate::blob::get_blob(tx, &fresh.blob_hash))
            .expect("non-utf8 bytes must still be durably stored as a blob");
        assert_eq!(blob, racer_bytes);
    }
}
