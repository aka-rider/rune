//! `Materialize` — the CAS write protocol that turns a buffer into the
//! user's destination file. Port of `pkg/docstate/materialize.go`, ported
//! "verbatim in shape" (plan WP4.S4): every step below either does `vfs`
//! I/O with no transaction open, or opens its own short
//! `retry::with_retry` transaction — no DB transaction is EVER held open
//! across a `vfs` call (plan binding rule, Go invariant I1).
//!
//! `content`/`expect`/`seq` are caller-captured parameters (from
//! `Store::materialize`'s enqueue-time snapshot of the buffer), never
//! re-derived inside — a later edit advancing the head while this op is in
//! flight must not silently claim the written bytes reflect edits they
//! don't (§1.4.2/§1.4.8).

use std::io;
use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, params};

use rune_vfs::Vfs;

use crate::Error;
use crate::adopt;
use crate::observation::{self, ObsId, Observation, ObservationMeta};
use crate::retry;

/// `doc_id`/`session_id` bundled together — every function in this module
/// needs both, and threading them as a pair (rather than two separate
/// parameters at every call site) is what keeps each signature under
/// clippy's argument-count lint without an `#[allow]` (repo rule: no such
/// allow outside test code).
#[derive(Clone, Copy, Debug)]
pub struct DocSession {
    pub doc_id: i64,
    pub session_id: i64,
}

/// The bytes to write and the journal position they correspond to, bundled
/// for the same argument-count reason — always passed together from
/// [`materialize`] down through [`materialize_overwrite`]/
/// [`materialize_create`]/[`commit_save`].
#[derive(Clone, Copy, Debug)]
struct WriteIntent<'a> {
    data: &'a [u8],
    seq: i64,
}

/// The caller-captured save intent [`materialize`] takes: `content` is what
/// to write, `expect` is the observation the caller last read as the
/// current disk fact (`SavedObs`, captured synchronously at save-start),
/// `seq` is the journal position `content` corresponds to, and `bind_new`
/// is the caller's explicit "a missing target is OK to create" intent.
/// Never re-derived once the op is enqueued (§1.4.2/§1.4.8).
#[derive(Clone, Copy, Debug)]
pub struct MaterializeInput<'a> {
    pub content: &'a str,
    pub expect: ObsId,
    pub seq: i64,
    pub bind_new: bool,
}

/// The outcome of [`materialize`]. Port of `materialize.go:157-187`
/// (`MatResult`) — Go's boolean-flagged always-present `Saved`/`Fresh`
/// fields become `Option` here (this crate's "Options for absent facts"
/// rule): `Missing`/`Fresh`-on-refusal/`Raced` stay mutually exclusive
/// discriminants, never a shared sentinel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MatResult {
    pub committed: bool,
    /// Meaningful when `committed` (an ordinary save OR a `raced` win).
    pub saved: Option<Observation>,
    /// Meaningful when `!committed && !missing`, OR `raced` (the
    /// displaced/conflicting observation).
    pub fresh: Option<Observation>,
    /// `true` when `!committed` because the target doesn't exist and
    /// `bind_new` was `false` (§1.4.4 — never silently (re)create).
    pub missing: bool,
    /// `true` when `committed` via a step-4 swap-race (F5): a writer raced
    /// inside the atomic-swap window, so the displaced bytes differ from
    /// `expect`, but OUR bytes are already physically at the target — this
    /// write commits for real, and the raced writer's displaced bytes are
    /// ALSO surfaced (`fresh`, `origin='swap'`).
    pub raced: bool,
}

/// Writes `input.content` to `doc_id`'s bound file under a CAS contract.
/// Port of `materialize.go:69-146` (`Materialize`).
pub fn materialize(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    ds: DocSession,
    path: &Path,
    input: MaterializeInput<'_>,
    now: SystemTime,
) -> Result<MatResult, Error> {
    let data = input.content.as_bytes();

    if input.bind_new {
        let resolved = vfs.resolve(path).map_err(Error::Io)?;
        if let Some(dir) = resolved.parent()
            && !dir.as_os_str().is_empty()
        {
            vfs.mkdir_all(dir).map_err(Error::Io)?;
        }
        return materialize_create(
            conn,
            vfs,
            ds,
            &resolved,
            WriteIntent {
                data,
                seq: input.seq,
            },
            now,
        );
    }

    let db_path: String = retry::with_retry(conn, |tx| {
        tx.query_row(
            "SELECT path FROM documents WHERE id=?1",
            params![ds.doc_id],
            |r| r.get(0),
        )
        .map_err(Error::from)
    })?;
    if db_path.is_empty() {
        return Err(Error::Invalid(format!(
            "materialize doc {}: no path bound (untitled document)",
            ds.doc_id
        )));
    }
    let resolved = vfs.resolve(Path::new(&db_path)).map_err(Error::Io)?;

    // Step 1: unconditional read+hash of the live target.
    let live_data = match vfs.read(&resolved) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // §1.4.4: an ordinary overwrite-intent save must never silently
            // (re)create a file the caller didn't explicitly ask to.
            return Ok(MatResult {
                missing: true,
                ..Default::default()
            });
        }
        Err(e) => return Err(Error::Io(e)),
    };

    let expect_obs = retry::with_retry(conn, |tx| observation::get_observation(tx, input.expect))?;

    // Step 2: live hash != expect -> refuse, no write.
    if observation::hash_bytes(&live_data) != expect_obs.blob_hash {
        let fresh = record_fresh(conn, vfs, ds, &resolved, &live_data, "probe", now)?;
        return Ok(MatResult {
            fresh: Some(fresh),
            ..Default::default()
        });
    }

    materialize_overwrite(
        conn,
        vfs,
        ds,
        &resolved,
        WriteIntent {
            data,
            seq: input.seq,
        },
        &expect_obs,
        now,
    )
}

/// Steps 3-5: once the pre-write hash confirmed the live target still
/// matches `expect`. Port of `materialize.go:148-203`
/// (`materializeOverwrite`).
fn materialize_overwrite(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    ds: DocSession,
    resolved: &Path,
    write: WriteIntent<'_>,
    expect_obs: &Observation,
    now: SystemTime,
) -> Result<MatResult, Error> {
    let temp = vfs.write_durable(resolved, write.data).map_err(Error::Io)?;

    if let Err(e) = vfs.exchange(&temp, resolved) {
        // Deliberately NOT removed (a documented strengthening over Go's
        // `materializeOverwrite`, which fire-and-forgets a cleanup here):
        // the publish never happened, `saved_obs` never moved, and the
        // temp is the ONLY place the user's just-written bytes still
        // physically exist outside the in-memory buffer/journal — orphaned
        // disk hygiene is a Tolerable cost, silently discarding a write
        // this function is about to report as failed is not (§1.4.10's
        // spirit, applied conservatively to the failure path too).
        return Err(Error::Io(e));
    }

    // Step 4: temp now holds what USED TO be at `resolved` (the displaced
    // bytes, never unlinked by the swap) — read+hash it.
    let displaced = vfs.read(&temp).map_err(Error::Io)?;
    if observation::hash_bytes(&displaced) != expect_obs.blob_hash {
        // F5 swap-race: a writer raced us inside the atomic-swap window.
        // The swap already physically happened — OUR bytes are what's
        // sitting at `resolved` right now. Capture the displaced bytes,
        // THEN commit our own write for real (the CAS record must match
        // physical reality), and remove the temp only after BOTH commit.
        let fresh = record_fresh(conn, vfs, ds, &temp, &displaced, "swap", now)?;
        let saved = commit_save(conn, vfs, ds, resolved, write, now)?;
        let _ = vfs.remove(&temp); // disk hygiene, not data safety — both records already committed
        return Ok(MatResult {
            committed: true,
            raced: true,
            saved: Some(saved),
            fresh: Some(fresh),
            missing: false,
        });
    }

    let saved = commit_save(conn, vfs, ds, resolved, write, now)?;
    // Only after the tx commits: remove the displaced-bytes temp (I1 —
    // never discard before the record commits).
    let _ = vfs.remove(&temp);
    Ok(MatResult {
        committed: true,
        saved: Some(saved),
        ..Default::default()
    })
}

/// Step 6: an atomic, no-clobber `rename_excl` for a bind-new or
/// recreate-after-delete target. Port of `materialize.go:205-233`
/// (`materializeCreate`).
fn materialize_create(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    ds: DocSession,
    resolved: &Path,
    write: WriteIntent<'_>,
    now: SystemTime,
) -> Result<MatResult, Error> {
    let temp = vfs.write_durable(resolved, write.data).map_err(Error::Io)?;
    if let Err(e) = vfs.rename_excl(&temp, resolved) {
        if e.kind() == io::ErrorKind::AlreadyExists {
            // A concurrent creator raced us — our own temp is genuinely
            // unneeded (the winner's bytes are what get recorded below),
            // safe to discard.
            let _ = vfs.remove(&temp);
            let live_data = vfs.read(resolved).map_err(Error::Io)?;
            let fresh = record_fresh(conn, vfs, ds, resolved, &live_data, "probe", now)?;
            return Ok(MatResult {
                fresh: Some(fresh),
                ..Default::default()
            });
        }
        // Deliberately NOT removed on a genuine I/O failure — see
        // `materialize_overwrite`'s matching comment: the temp is the only
        // place the user's bytes still physically exist.
        return Err(Error::Io(e));
    }
    let saved = commit_save(conn, vfs, ds, resolved, write, now)?;
    Ok(MatResult {
        committed: true,
        saved: Some(saved),
        ..Default::default()
    })
}

/// Puts `data`'s raw bytes as a blob and records an observation of it at
/// `path`'s current stat, for the `Conflict{Fresh}` outcomes. `data` is
/// disk-sourced — the target's live content on a CAS refusal, or a racer's
/// displaced bytes on a swap-race (§1.4.10 mandates this capture happens
/// unconditionally, never gated on UTF-8 validity: see `blob.rs`'s module
/// doc). Port of `materialize.go:235-243` (`recordFresh`).
pub(crate) fn record_fresh(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    ds: DocSession,
    path: &Path,
    data: &[u8],
    origin: &str,
    now: SystemTime,
) -> Result<Observation, Error> {
    let hash = retry::with_retry(conn, |tx| crate::blob::put_blob(tx, data))?;
    observation::observe_from_stat(
        conn,
        vfs,
        ds.session_id,
        ds.doc_id,
        path,
        ObservationMeta {
            blob_hash: &hash,
            seq: None,
            origin,
        },
        now,
    )
}

/// Step 5: ONE tx — observation(`origin='save'`, hash of the bytes WE
/// WROTE) + `saved_obs` update + re-Bind (path/inode/device/`kind='file'`,
/// post-swap stat). `write.seq` is the caller's save-start-captured journal
/// position — NEVER re-read here. The post-write stat (disk I/O) happens
/// BEFORE the tx opens; the tx itself is pure SQLite (I1's
/// no-tx-across-disk-I/O contract). Port of `materialize.go:245-325`
/// (`commitSave`).
fn commit_save(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    ds: DocSession,
    resolved: &Path,
    write: WriteIntent<'_>,
    now: SystemTime,
) -> Result<Observation, Error> {
    let hash = retry::with_retry(conn, |tx| crate::blob::put_blob(tx, write.data))?;

    let stat = observation::stat_identity(vfs, resolved);
    let at = crate::session::format_rfc3339_nanos(now);
    let resolved_str = resolved.to_string_lossy().into_owned();

    retry::with_retry(conn, |tx| {
        let obs = adopt::record_adoption_tx(
            tx,
            ds.doc_id,
            ds.session_id,
            ObservationMeta {
                blob_hash: &hash,
                seq: Some(write.seq),
                origin: "save",
            },
            &stat,
            &at,
        )?;

        rebind_document_tx(
            tx,
            ds.doc_id,
            Rebind {
                path: &resolved_str,
                stat: &stat,
                at: &at,
            },
        )?;

        Ok(obs)
    })
}

/// The path/identity half of a document rebind, bundled for the same
/// argument-count reason as [`DocSession`]/[`WriteIntent`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rebind<'a> {
    /// The destination path, already `vfs.resolve`d and stringified.
    pub path: &'a str,
    /// The destination's post-publish stat. `inode.is_some()` is what gates
    /// the identity-steal statement — a backend that exposes no inode must
    /// not blank every other row's `NULL` identity as if it matched.
    pub stat: &'a observation::StatFacts,
    /// RFC3339-nanos timestamp for `last_seen_at`.
    pub at: &'a str,
}

/// Points `doc_id`'s `documents` row at `rebind.path` + its on-disk
/// identity, evicting any OTHER row that currently claims that path or that
/// (inode, device) — §1.7's one-value-one-meaning applied to the
/// path/identity columns: two rows must never both claim the same file.
///
/// Extracted verbatim from [`commit_save`] (plan step 2, "zero behavior
/// change") because a rename needs exactly this and nothing else around it.
/// A rename must NOT go through `commit_save`: that also does
/// `put_blob(write.data)` + `record_adoption_tx(origin='save')`, which after
/// renaming a *dirty* document would move `saved_obs` to an observation
/// claiming the disk holds the journal head. The next ⌘S would then CAS
/// against a lie (§1.4.7).
///
/// Caller-supplied transaction: this is pure SQLite with no `vfs` call
/// inside, so it is safe to run under an open tx (invariant I1).
pub(crate) fn rebind_document_tx(
    tx: &Connection,
    doc_id: i64,
    rebind: Rebind<'_>,
) -> Result<(), Error> {
    let stat = rebind.stat;

    tx.execute(
        "UPDATE documents SET path='' WHERE path=?1 AND id!=?2",
        params![rebind.path, doc_id],
    )?;

    if stat.inode.is_some() {
        tx.execute(
            "UPDATE documents SET inode=NULL, device=NULL WHERE inode=?1 AND device=?2 AND id!=?3",
            params![stat.inode, stat.device, doc_id],
        )?;
    }

    tx.execute(
        "UPDATE documents SET path=?1, inode=?2, device=?3, kind='file', last_seen_at=?4 WHERE id=?5",
        params![rebind.path, stat.inode, stat.device, rebind.at, doc_id],
    )?;

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_vfs::{Mem, OpKind as VfsOp};

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

    /// A live target that no longer matches `expect` must refuse the write
    /// entirely — no `write_durable`/`exchange` call ever reaches the vfs
    /// (proven via `Mem::fail_next`, which would only matter if the write
    /// path were reached: if it were, this test would still pass with the
    /// write silently landing — the real assertion is `committed==false`
    /// with `fresh` populated and disk content unchanged).
    #[test]
    fn cas_refusal_on_external_change_makes_no_write() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"external content");
        let doc_id = seed_doc_with_path(&conn, "/doc.md");
        let expect = record_obs(&conn, doc_id, session_id, "stale-hash-not-matching-disk");

        // Arm a failure on the next write_durable: if materialize somehow
        // attempted a write despite the CAS mismatch, this test would fail
        // loudly instead of silently succeeding.
        vfs.fail_next(VfsOp::WriteDurable, io::ErrorKind::Other);

        let result = materialize(
            &mut conn,
            &vfs,
            DocSession { doc_id, session_id },
            path,
            MaterializeInput {
                content: "our content",
                expect,
                seq: 1,
                bind_new: false,
            },
            SystemTime::now(),
        )
        .expect("materialize must not error, only refuse");

        assert!(!result.committed);
        assert!(result.fresh.is_some());
        assert!(!result.missing);

        let disk = vfs.read(path).expect("disk still has external content");
        assert_eq!(disk, b"external content");
    }

    /// A save-start CAS `expect` matching live disk AT THE TIME IT WAS
    /// CAPTURED, but a racer landing different bytes at `resolved` in the
    /// window before OUR `exchange` runs, must still commit OUR write (the
    /// swap is atomic and already physically happened) AND capture the
    /// raced writer's displaced bytes as a durable blob/observation
    /// (`origin='swap'`). `Mem` is single-threaded/synchronous, so this
    /// exercises `materialize_overwrite` (the private step-3-5 primitive)
    /// directly rather than the public `materialize` entry point: it calls
    /// in AFTER the racer has already landed its bytes at `resolved` but
    /// with `expect_obs` still reflecting what `Materialize`'s own step 1-2
    /// CAS check would have captured a moment earlier — precisely the
    /// window F5 describes.
    #[test]
    fn swap_race_captures_displaced_bytes_as_a_blob_and_commits_raced() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"original");
        let doc_id = seed_doc_with_path(&conn, "/doc.md");

        let expect_obs = Observation {
            id: 0,
            doc_id,
            session_id,
            blob_hash: observation::hash_bytes(b"original"),
            seq: None,
            size: 0,
            mtime: String::new(),
            inode: None,
            device: None,
            nlink: None,
            origin: "load".to_string(),
            supersedes: None,
            at: String::new(),
        };

        // The racer lands its own bytes at `path` in the window between
        // Materialize's CAS check and our own `exchange` call.
        let racer_temp = vfs
            .write_durable(path, b"racer bytes")
            .expect("racer write_durable");
        vfs.exchange(&racer_temp, path).expect("racer exchange");
        vfs.remove(&racer_temp).ok();

        let result = materialize_overwrite(
            &mut conn,
            &vfs,
            DocSession { doc_id, session_id },
            path,
            WriteIntent {
                data: b"our content",
                seq: 1,
            },
            &expect_obs,
            SystemTime::now(),
        )
        .expect("materialize_overwrite");

        assert!(result.committed, "our write must commit despite the race");
        assert!(result.raced, "must be flagged as a swap-race win");
        let fresh = result.fresh.expect("displaced bytes must be captured");
        assert_eq!(fresh.origin, "swap");
        assert_eq!(fresh.blob_hash, observation::hash_bytes(b"racer bytes"));

        // The blob is durably retrievable.
        let blob = retry::with_retry(&mut conn, |tx| crate::blob::get_blob(tx, &fresh.blob_hash))
            .expect("racer bytes durably stored as a blob");
        assert_eq!(blob, b"racer bytes");

        // OUR bytes are what's physically on disk now.
        let disk = vfs.read(path).expect("read disk");
        assert_eq!(disk, b"our content");
    }

    /// §1.4.10 mandates capturing displaced bytes unconditionally — even
    /// when the racer's bytes are NOT valid UTF-8 (e.g. a binary file, or
    /// another process's own in-progress non-text write landing in the
    /// swap window). Before the blob layer was retyped to raw bytes, this
    /// hit `std::str::from_utf8` inside `record_fresh` and hard-errored:
    /// no blob, no `commit_save`, even though OUR bytes had already
    /// physically swapped in — exactly the failure mode this test guards
    /// against.
    #[test]
    fn swap_race_with_non_utf8_racer_bytes_commits_raced_and_captures_the_blob_byte_exact() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"original");
        let doc_id = seed_doc_with_path(&conn, "/doc.md");

        let expect_obs = Observation {
            id: 0,
            doc_id,
            session_id,
            blob_hash: observation::hash_bytes(b"original"),
            seq: None,
            size: 0,
            mtime: String::new(),
            inode: None,
            device: None,
            nlink: None,
            origin: "load".to_string(),
            supersedes: None,
            at: String::new(),
        };

        // Non-UTF-8 racer bytes land at `path` in the swap window (0xFF is
        // never a valid UTF-8 lead byte).
        let racer_bytes: &[u8] = &[0xff, 0xfe, 0x00, 0x9f, 0x92, 0x96, 0x80];
        let racer_temp = vfs
            .write_durable(path, racer_bytes)
            .expect("racer write_durable");
        vfs.exchange(&racer_temp, path).expect("racer exchange");
        vfs.remove(&racer_temp).ok();

        let result = materialize_overwrite(
            &mut conn,
            &vfs,
            DocSession { doc_id, session_id },
            path,
            WriteIntent {
                data: b"our content",
                seq: 1,
            },
            &expect_obs,
            SystemTime::now(),
        )
        .expect("materialize_overwrite must commit, not hard-error, on non-utf8 displaced bytes");

        assert!(result.committed, "our write must commit despite the race");
        assert!(result.raced, "must be flagged as a swap-race win");
        let fresh = result.fresh.expect("displaced bytes must be captured");
        assert_eq!(fresh.origin, "swap");
        assert_eq!(fresh.blob_hash, observation::hash_bytes(racer_bytes));

        let blob = retry::with_retry(&mut conn, |tx| crate::blob::get_blob(tx, &fresh.blob_hash))
            .expect("non-utf8 racer bytes must still be durably stored as a blob");
        assert_eq!(
            blob, racer_bytes,
            "displaced bytes must round-trip byte-exact, even though not valid UTF-8"
        );

        let disk = vfs.read(path).expect("read disk");
        assert_eq!(disk, b"our content");
    }

    /// `Mem::fail_next(Exchange)` mid-materialize: the error must surface,
    /// `saved_obs` must not move, and the temp file must still hold the
    /// user's bytes (never silently lost).
    #[test]
    fn exchange_failure_surfaces_error_no_saved_obs_move_temp_keeps_bytes() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"original");
        let doc_id = seed_doc_with_path(&conn, "/doc.md");
        let expect = record_obs(&conn, doc_id, session_id, "original");

        vfs.fail_next(VfsOp::Exchange, io::ErrorKind::Other);

        let err = materialize(
            &mut conn,
            &vfs,
            DocSession { doc_id, session_id },
            path,
            MaterializeInput {
                content: "user bytes",
                expect,
                seq: 1,
                bind_new: false,
            },
            SystemTime::now(),
        )
        .expect_err("exchange failure must surface");
        assert!(matches!(err, Error::Io(_)));

        let saved_obs_row_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_documents WHERE session_id=?1 AND doc_id=?2)",
                params![session_id, doc_id],
                |r| r.get(0),
            )
            .expect("check saved_obs existence");
        assert!(
            !saved_obs_row_exists,
            "saved_obs must not move on a failed exchange"
        );

        // The temp write_durable produced must still physically hold the
        // user's bytes — never silently discarded on this failure path.
        let temps: Vec<_> = vfs
            .debug_paths()
            .into_iter()
            .filter(|p| p.as_path() != path)
            .collect();
        assert_eq!(
            temps.len(),
            1,
            "exactly one orphaned temp must remain: {temps:?}"
        );
        let temp_bytes = vfs.read(&temps[0]).expect("temp still readable");
        assert_eq!(temp_bytes, b"user bytes");
    }

    /// A missing target with `bind_new=false` must refuse with `missing`,
    /// never silently create the file.
    #[test]
    fn missing_target_without_bind_new_refuses_as_missing() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc_with_path(&conn, "/gone.md");
        let expect = record_obs(&conn, doc_id, session_id, "irrelevant");

        let result = materialize(
            &mut conn,
            &vfs,
            DocSession { doc_id, session_id },
            Path::new("/gone.md"),
            MaterializeInput {
                content: "content",
                expect,
                seq: 1,
                bind_new: false,
            },
            SystemTime::now(),
        )
        .expect("materialize must not error");
        assert!(!result.committed);
        assert!(result.missing);
    }

    /// `bind_new=true` on a brand-new path creates it atomically via
    /// `rename_excl`.
    #[test]
    fn bind_new_creates_the_file_atomically() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc_with_path(&conn, "");

        let result = materialize(
            &mut conn,
            &vfs,
            DocSession { doc_id, session_id },
            Path::new("/new.md"),
            MaterializeInput {
                content: "brand new content",
                expect: 0,
                seq: 1,
                bind_new: true,
            },
            SystemTime::now(),
        )
        .expect("materialize");
        assert!(result.committed);
        assert!(result.saved.is_some());

        let disk = vfs.read(Path::new("/new.md")).expect("file created");
        assert_eq!(disk, b"brand new content");
    }

    /// Port of Go `materialize_test.go:278-317` parity (finding 9): a
    /// concurrent creator publishes the target BEFORE our own `bind_new`
    /// materialize's `rename_excl` runs. We must refuse (never clobber the
    /// winner), record a fresh `origin='probe'` observation of the winner's
    /// actual bytes, and leave the winner's bytes on disk untouched.
    #[test]
    fn bind_new_create_race_refuses_and_records_winners_bytes() {
        let mut conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let doc_id = seed_doc_with_path(&conn, "");
        let path = Path::new("/new.md");

        // A concurrent creator wins the race and publishes first.
        publish(&vfs, path, b"winner's bytes");

        let result = materialize(
            &mut conn,
            &vfs,
            DocSession { doc_id, session_id },
            path,
            MaterializeInput {
                content: "our content",
                expect: 0,
                seq: 1,
                bind_new: true,
            },
            SystemTime::now(),
        )
        .expect("materialize must not error, only refuse");

        assert!(!result.committed, "the create must be refused");
        let fresh = result.fresh.expect("winner's bytes must be recorded");
        assert_eq!(fresh.origin, "probe");
        assert_eq!(fresh.blob_hash, observation::hash_bytes(b"winner's bytes"));

        let saved_obs_row_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_documents WHERE session_id=?1 AND doc_id=?2)",
                params![session_id, doc_id],
                |r| r.get(0),
            )
            .expect("check saved_obs existence");
        assert!(
            !saved_obs_row_exists,
            "a refused create must never move saved_obs"
        );

        let disk = vfs.read(path).expect("winner's file still on disk");
        assert_eq!(disk, b"winner's bytes", "the winner's bytes must be intact");
    }
}
