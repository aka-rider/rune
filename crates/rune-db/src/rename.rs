//! Rename — moving a bound document from one path to another, and the
//! destructive `[R]eplace` variant that renames *onto* a file that already
//! exists.
//!
//! Two entry points, each a single writer op that is **never split across a
//! message boundary**:
//!
//! - [`rename_bind`] — the non-destructive case: `renamex_np(RENAME_EXCL)`
//!   (§1.4.1's no-clobber atomic publish). If the destination exists it
//!   returns [`RenameOutcome::Collided`] having written *nothing*, and the
//!   UI raises the guard §1.4.4 requires before a destructive transition.
//! - [`rename_replace`] — the confirmed-destructive case. It is
//!   capture-then-swap-then-commit-then-unlink, in that order, so §1.4.10's
//!   "capture before discard — physically" holds by mechanism: the replaced
//!   file's bytes are a durable blob before its last name is unlinked. That
//!   blob is the only record the replaced file ever existed, since rune
//!   never opened it.
//!
//! Why the two halves are ONE op each: the capture and the swap cannot be
//! separated by a message round-trip without making "swapped but not
//! captured" a representable state — the exact state §1.4.10 forbids.
//!
//! ### What a rename deliberately does NOT do
//!
//! - **No save.** §1.4.2 names the only two acts that touch the destination
//!   (⌘S and save-on-close); a rename is neither. A dirty document renames
//!   and stays dirty — §1.4.6 keys history to inode+device and `renamex_np`
//!   preserves the inode, so no history is orphaned.
//! - **No observation for a plain rename.** `observations` (`schema.rs`)
//!   has no path column: it records blob_hash/size/mtime/inode/device/seq,
//!   none of which `renamex_np` changes. The existing `saved_obs` stays
//!   exactly as valid, and a spurious new one would move the CAS baseline.
//! - **No `commit_save`.** It would `put_blob(buffer)` +
//!   `record_adoption_tx(origin='save')`, i.e. claim the disk holds the
//!   journal head. After renaming a *dirty* document the next ⌘S would then
//!   CAS against a lie (§1.4.7). Only `rebind_document_tx` is reused.
//!
//! ### Failure atomicity
//!
//! No edge loses bytes. `rename_excl`/`exchange` are single kernel
//! operations, so a failure there leaves both paths intact. After the swap,
//! the displaced bytes live at `from` and are deliberately **not** removed
//! on any failure path — the error text names `from` so the user can
//! recover them by hand, the same doctrine as `materialize`'s
//! deliberately-unremoved temp. The only lossy operation in the whole
//! design is the final `remove(from)`, which runs strictly after both the
//! blob and the rebind have committed.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rune_vfs::{Stat, Vfs};

use crate::Error;
use crate::materialize::{DocSession, Rebind, rebind_document_tx};
use crate::observation::{self, Observation, ObserveInput};
use crate::retry;

/// The outcome of [`rename_bind`] / [`rename_replace`].
///
/// `Collided` and `Refused` are refusals, not errors: nothing on disk or in
/// the database changed, and the UI turns each into a prompt or a message
/// rather than a halt.
#[derive(Clone, Debug, PartialEq)]
pub enum RenameOutcome {
    /// The document is now bound to `to`. Its dirty state and `saved_obs`
    /// are unchanged — a rename is not a save.
    Renamed { to: PathBuf },
    /// `to` already exists. **Nothing was written**, to disk or to the
    /// database. `seen` is what the destination looked like at the moment
    /// of the collision; it becomes the consent baseline the user is shown
    /// and that [`rename_replace`] re-checks.
    Collided { seen: Stat },
    /// The replace committed. `displaced` is the observation of the bytes
    /// that used to live at the destination, already durably captured as a
    /// blob (§1.4.10) — `displaced.blob_hash` retrieves them via
    /// `get_blob`.
    Replaced { displaced: Observation },
    /// The destination no longer matches the `seen` the user consented to,
    /// so the replace was abandoned before touching anything. `fresh` is
    /// what the destination looks like now.
    Refused { fresh: Stat },
}

/// Renames `from` to `to` with no clobber, then rebinds the document row.
///
/// Sequence — no transaction is ever open across a `vfs` call (invariant
/// I1):
/// 1. `rename_excl(from, to)`. `AlreadyExists` → stat `to` and return
///    `Collided` with **no** database write at all.
/// 2. `stat_identity(to)` — disk I/O, outside any transaction.
/// 3. One transaction: `rebind_document_tx`.
///
/// If step 3 fails the file is at `to` while the database still says
/// `from`. That degrades safely: the next ⌘S CASes against a path that no
/// longer exists and refuses with `MatResult{missing}` rather than writing
/// anywhere unexpected.
pub fn rename_bind(
    conn: &mut rusqlite::Connection,
    vfs: &dyn Vfs,
    ds: DocSession,
    from: &Path,
    to: &Path,
    now: SystemTime,
) -> Result<RenameOutcome, Error> {
    if let Err(e) = vfs.rename_excl(from, to) {
        if e.kind() == io::ErrorKind::AlreadyExists {
            let seen = vfs.stat(to).map_err(Error::Io)?;
            return Ok(RenameOutcome::Collided { seen });
        }
        return Err(Error::Io(e));
    }

    rebind(conn, vfs, ds, to, now)?;
    Ok(RenameOutcome::Renamed {
        to: to.to_path_buf(),
    })
}

/// Renames `from` onto the existing file at `to`, preserving the replaced
/// file's bytes as a durable blob first.
///
/// `seen` is the [`RenameOutcome::Collided`] stat the user was shown. The
/// re-stat below is a **consent** check — "is this still the file you
/// agreed to replace?" — and explicitly *not* the safety mechanism. Safety
/// comes from step 3: the displaced bytes are read back **after** the
/// atomic swap, so even a writer that raced inside the swap window is
/// captured for whatever the file actually was at the instant it was
/// displaced. That is the same shape `materialize_overwrite` uses.
///
/// Sequence:
/// 1. `stat(to)`; ≠ `seen` → `Refused`, nothing touched.
/// 2. `exchange(from, to)` — atomic. `to` now holds our file object (our
///    inode travels, exactly as `rename_excl` would have moved it) and
///    `from` holds the displaced file. Neither path is unlinked.
/// 3. `read(from)` → the displaced bytes.
/// 4. ONE transaction (`capture_and_rebind`): puts the displaced bytes as a
///    blob, records the `origin='swap'` observation referencing it, AND
///    rebinds the document row to `to` — all three commit together or none
///    do. Previously the swap-observation and the rebind were two separate
///    transactions, so a crash between them left the observation committed
///    but the document row still naming `from` with the OLD identity —
///    reopening `from` would then stat the now-foreign inode sitting there,
///    miss the identity lookup, and blank our own row before minting a
///    historyless one for the foreign file ([rune-db 4]). Collapsing both
///    into one transaction makes that intermediate state unreachable: a
///    crash here now either leaves NEITHER committed (rolled back, exactly
///    as if step 4 had not started) or BOTH.
/// 5. `remove(from)` — **only now**, after the transaction committed.
///
/// `origin='swap'` is reused rather than a new `'displaced'` value on
/// purpose: `schema.rs`'s `CHECK(origin IN (...))` would need a
/// `SCHEMA_VERSION` bump, and migration is drop-on-mismatch — it would
/// throw away every user's unsaved-work journal on upgrade. `'swap'`
/// already means "bytes an atomic `exchange` displaced", which is literally
/// the mechanism here.
pub fn rename_replace(
    conn: &mut rusqlite::Connection,
    vfs: &dyn Vfs,
    ds: DocSession,
    from: &Path,
    to: &Path,
    seen: Stat,
    now: SystemTime,
) -> Result<RenameOutcome, Error> {
    // 1. Consent re-check. A failure to stat is itself a refusal-shaped
    //    situation, but we surface it as an error rather than inventing a
    //    `fresh` we never saw.
    let fresh = vfs.stat(to).map_err(Error::Io)?;
    if fresh != seen {
        return Ok(RenameOutcome::Refused { fresh });
    }

    // 2. The atomic publish (§1.4.1). Both files still exist afterwards,
    //    with their contents swapped.
    vfs.exchange(from, to).map_err(Error::Io)?;

    // 3. The displaced bytes are now at `from`. Read them AFTER the swap.
    let displaced_bytes = vfs.read(from).map_err(|e| {
        // Name `from` explicitly: our content is at `to` and the replaced
        // file's only remaining copy is at `from`, un-captured. The user
        // must be told exactly where it is.
        Error::Io(io::Error::new(
            e.kind(),
            format!(
                "renamed onto {}, but could not read the displaced bytes back from {} \
                 to preserve them — they are still on disk at that path: {e}",
                to.display(),
                from.display()
            ),
        ))
    })?;

    // 4. Capture before discard, physically (§1.4.10), AND rebind — in ONE
    //    transaction (see the doc comment above): if this fails, NEITHER the
    //    observation nor the rebind took effect, our content is at `to`, the
    //    database still says `from` with its OLD identity, and `from` holds
    //    the foreign bytes untouched — a later ⌘S hashes those foreign
    //    bytes, mismatches `expect_obs`, and refuses. So `from` is
    //    deliberately NOT removed here.
    let displaced = capture_and_rebind(conn, vfs, ds, from, to, &displaced_bytes, now)?;

    // 5. The only lossy step in the design, strictly after the transaction
    //    committed. A failure here is disk hygiene, not data safety
    //    (§0.1 rung 3): the blob is already durable.
    let _ = vfs.remove(from);

    Ok(RenameOutcome::Replaced { displaced })
}

/// The step-4 primitive `rename_replace` calls: puts `displaced_bytes` as a
/// blob, records the `origin='swap'` observation of them (captured at
/// `from`, where the displaced file object now lives), and rebinds the
/// document row to `to` — all inside ONE transaction, closing the crash
/// window [rune-db 4] describes. Both stats (disk I/O) run BEFORE the
/// transaction opens (invariant I1); the transaction itself is pure SQLite.
fn capture_and_rebind(
    conn: &mut rusqlite::Connection,
    vfs: &dyn Vfs,
    ds: DocSession,
    from: &Path,
    to: &Path,
    displaced_bytes: &[u8],
    now: SystemTime,
) -> Result<Observation, Error> {
    let from_stat = observation::stat_identity(vfs, from);
    let to_stat = observation::stat_identity(vfs, to);
    let at = crate::session::format_rfc3339_nanos(now);
    let to_str = crate::paths::to_db_string(to)?;

    retry::with_retry(conn, |tx| {
        let displaced = observation::observe_from_stat_tx(
            tx,
            ds.session_id,
            ds.doc_id,
            &from_stat,
            &at,
            ObserveInput {
                data: displaced_bytes,
                seq: None,
                origin: "swap",
            },
        )?;

        rebind_document_tx(
            tx,
            ds.doc_id,
            Rebind {
                path: &to_str,
                stat: &to_stat,
                at: &at,
            },
        )?;

        Ok(displaced)
    })
}

/// Stat `to` (disk I/O, no transaction open) and point the document row at
/// it in one short transaction (invariant I1).
fn rebind(
    conn: &mut rusqlite::Connection,
    vfs: &dyn Vfs,
    ds: DocSession,
    to: &Path,
    now: SystemTime,
) -> Result<(), Error> {
    let stat = observation::stat_identity(vfs, to);
    let at = crate::session::format_rfc3339_nanos(now);
    let to_str = crate::paths::to_db_string(to)?;

    retry::with_retry(conn, |tx| {
        rebind_document_tx(
            tx,
            ds.doc_id,
            Rebind {
                path: &to_str,
                stat: &stat,
                at: &at,
            },
        )
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use rune_vfs::{Mem, OpKind as VfsOp};
    use rusqlite::{Connection, params};

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

    fn obs_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .expect("count observations")
    }

    fn doc_path(conn: &Connection, doc_id: i64) -> String {
        conn.query_row(
            "SELECT path FROM documents WHERE id=?1",
            params![doc_id],
            |r| r.get(0),
        )
        .expect("doc path")
    }

    struct Fixture {
        conn: Connection,
        vfs: Mem,
        ds: DocSession,
    }

    /// A document bound to `/a.md` holding `content`.
    fn fixture(content: &[u8]) -> Fixture {
        let conn = open();
        let vfs = Mem::new();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("establish session");
        publish(&vfs, Path::new("/a.md"), content);
        let doc_id = seed_doc_with_path(&conn, "/a.md");
        Fixture {
            conn,
            vfs,
            ds: DocSession { doc_id, session_id },
        }
    }

    /// The happy path: the file moves, the row is rebound, identity is
    /// preserved, and — critically — **no observation is recorded**, so the
    /// CAS baseline a later ⌘S uses is untouched.
    #[test]
    fn rename_bind_moves_the_file_rebinds_the_row_and_records_no_observation() {
        let mut f = fixture(b"hello");
        let before = f.vfs.stat(Path::new("/a.md")).expect("stat before");
        let obs_before = obs_count(&f.conn);

        let out = rename_bind(
            &mut f.conn,
            &f.vfs,
            f.ds,
            Path::new("/a.md"),
            Path::new("/b.md"),
            SystemTime::now(),
        )
        .expect("rename_bind");

        assert_eq!(
            out,
            RenameOutcome::Renamed {
                to: PathBuf::from("/b.md")
            }
        );
        assert_eq!(f.vfs.read(Path::new("/b.md")).expect("read to"), b"hello");
        assert!(f.vfs.read(Path::new("/a.md")).is_err(), "from must be gone");
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");

        let after = f.vfs.stat(Path::new("/b.md")).expect("stat after");
        assert_eq!(
            after.identity, before.identity,
            "renamex_np preserves inode+device (§1.4.6)"
        );
        assert_eq!(
            obs_count(&f.conn),
            obs_before,
            "a plain rename must record no observation"
        );
    }

    /// A collision writes nothing anywhere and reports exactly what the
    /// destination looks like.
    #[test]
    fn rename_bind_collision_writes_nothing_and_reports_the_destination_stat() {
        let mut f = fixture(b"ours");
        publish(&f.vfs, Path::new("/b.md"), b"theirs");
        let obs_before = obs_count(&f.conn);
        let expected = f.vfs.stat(Path::new("/b.md")).expect("stat b");

        let out = rename_bind(
            &mut f.conn,
            &f.vfs,
            f.ds,
            Path::new("/a.md"),
            Path::new("/b.md"),
            SystemTime::now(),
        )
        .expect("collision is a refusal, not an error");

        assert_eq!(out, RenameOutcome::Collided { seen: expected });
        assert_eq!(f.vfs.read(Path::new("/a.md")).expect("a intact"), b"ours");
        assert_eq!(f.vfs.read(Path::new("/b.md")).expect("b intact"), b"theirs");
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/a.md");
        assert_eq!(obs_count(&f.conn), obs_before);
    }

    /// A genuine `rename_excl` failure surfaces as an error with both paths
    /// intact and the database untouched — `renamex_np` is atomic, so there
    /// is no half-renamed state to clean up.
    #[test]
    fn rename_bind_io_failure_surfaces_with_both_paths_intact() {
        let mut f = fixture(b"ours");
        f.vfs
            .fail_next(VfsOp::RenameExcl, io::ErrorKind::PermissionDenied);

        let err = rename_bind(
            &mut f.conn,
            &f.vfs,
            f.ds,
            Path::new("/a.md"),
            Path::new("/b.md"),
            SystemTime::now(),
        )
        .expect_err("permission denied must surface");
        assert!(matches!(err, Error::Io(_)));

        assert_eq!(f.vfs.read(Path::new("/a.md")).expect("a intact"), b"ours");
        assert!(f.vfs.read(Path::new("/b.md")).is_err());
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/a.md");
    }

    /// The full replace: our bytes land at `to`, `from` is gone, and the
    /// replaced file's bytes are retrievable from the blob store — the only
    /// record that file ever existed.
    #[test]
    fn rename_replace_preserves_the_displaced_bytes_as_a_durable_blob() {
        let mut f = fixture(b"ours");
        publish(&f.vfs, Path::new("/b.md"), b"theirs");
        let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");

        let out = rename_replace(
            &mut f.conn,
            &f.vfs,
            f.ds,
            Path::new("/a.md"),
            Path::new("/b.md"),
            seen,
            SystemTime::now(),
        )
        .expect("rename_replace");

        let RenameOutcome::Replaced { displaced } = out else {
            panic!("expected Replaced, got {out:?}");
        };
        assert_eq!(displaced.doc_id, f.ds.doc_id, "captured under OUR doc");
        assert_eq!(displaced.origin, "swap");
        assert_eq!(
            displaced.blob_hash,
            observation::hash_bytes(b"theirs"),
            "the blob must be the REPLACED file's bytes"
        );

        let blob = retry::with_retry(&mut f.conn, |tx| {
            crate::blob::get_blob(tx, &displaced.blob_hash)
        })
        .expect("displaced bytes durably stored");
        assert_eq!(blob, b"theirs");

        assert_eq!(f.vfs.read(Path::new("/b.md")).expect("read to"), b"ours");
        assert!(f.vfs.read(Path::new("/a.md")).is_err(), "from removed");
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");
    }

    /// §1.4.10 capture is unconditional — never gated on UTF-8 validity.
    /// The replaced file was never opened by rune, so it can be anything.
    #[test]
    fn rename_replace_captures_non_utf8_displaced_bytes_byte_exact() {
        let mut f = fixture(b"ours");
        let theirs: &[u8] = &[0xff, 0xfe, 0x00, 0x9f, 0x92, 0x96, 0x80];
        publish(&f.vfs, Path::new("/b.md"), theirs);
        let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");

        let out = rename_replace(
            &mut f.conn,
            &f.vfs,
            f.ds,
            Path::new("/a.md"),
            Path::new("/b.md"),
            seen,
            SystemTime::now(),
        )
        .expect("non-utf8 displaced bytes must not hard-error");

        let RenameOutcome::Replaced { displaced } = out else {
            panic!("expected Replaced, got {out:?}");
        };
        let blob = retry::with_retry(&mut f.conn, |tx| {
            crate::blob::get_blob(tx, &displaced.blob_hash)
        })
        .expect("blob");
        assert_eq!(blob, theirs, "displaced bytes must round-trip byte-exact");
    }

    /// The consent check: the destination changed since the user was shown
    /// it, so nothing is touched at all.
    #[test]
    fn rename_replace_refuses_when_the_destination_changed_since_consent() {
        let mut f = fixture(b"ours");
        publish(&f.vfs, Path::new("/b.md"), b"theirs");
        let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");
        let obs_before = obs_count(&f.conn);

        // Someone else rewrites the destination after the user agreed.
        let temp = f
            .vfs
            .write_durable(Path::new("/b.md"), b"changed underneath")
            .expect("racer write");
        f.vfs
            .exchange(&temp, Path::new("/b.md"))
            .expect("racer exchange");
        f.vfs.remove(&temp).ok();

        let out = rename_replace(
            &mut f.conn,
            &f.vfs,
            f.ds,
            Path::new("/a.md"),
            Path::new("/b.md"),
            seen,
            SystemTime::now(),
        )
        .expect("a refusal is not an error");

        assert!(matches!(out, RenameOutcome::Refused { .. }));
        assert_eq!(f.vfs.read(Path::new("/a.md")).expect("a intact"), b"ours");
        assert_eq!(
            f.vfs.read(Path::new("/b.md")).expect("b intact"),
            b"changed underneath"
        );
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/a.md");
        assert_eq!(obs_count(&f.conn), obs_before, "no blob, no rebind");
    }

    /// A failed `exchange` leaves both files exactly where they were — it
    /// is one kernel operation, so there is no partial state.
    #[test]
    fn rename_replace_exchange_failure_leaves_both_files_intact() {
        let mut f = fixture(b"ours");
        publish(&f.vfs, Path::new("/b.md"), b"theirs");
        let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");
        let obs_before = obs_count(&f.conn);
        f.vfs.fail_next(VfsOp::Exchange, io::ErrorKind::Other);

        let err = rename_replace(
            &mut f.conn,
            &f.vfs,
            f.ds,
            Path::new("/a.md"),
            Path::new("/b.md"),
            seen,
            SystemTime::now(),
        )
        .expect_err("exchange failure must surface");
        assert!(matches!(err, Error::Io(_)));

        assert_eq!(f.vfs.read(Path::new("/a.md")).expect("a intact"), b"ours");
        assert_eq!(f.vfs.read(Path::new("/b.md")).expect("b intact"), b"theirs");
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/a.md");
        assert_eq!(obs_count(&f.conn), obs_before);
    }

    /// The post-swap read is the one step that can fail with the swap
    /// already done. The displaced bytes must still be physically present
    /// at `from` — never removed on this path — and the error must say so.
    #[test]
    fn rename_replace_post_swap_read_failure_leaves_the_displaced_bytes_at_from() {
        let mut f = fixture(b"ours");
        publish(&f.vfs, Path::new("/b.md"), b"theirs");
        let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");
        f.vfs.fail_next(VfsOp::Read, io::ErrorKind::Other);

        let err = rename_replace(
            &mut f.conn,
            &f.vfs,
            f.ds,
            Path::new("/a.md"),
            Path::new("/b.md"),
            seen,
            SystemTime::now(),
        )
        .expect_err("post-swap read failure must surface");
        let msg = err.to_string();
        assert!(
            msg.contains("/a.md"),
            "the error must name where the displaced bytes still are: {msg}"
        );

        assert_eq!(
            f.vfs
                .read(Path::new("/a.md"))
                .expect("displaced bytes kept"),
            b"theirs",
            "the replaced file's bytes must still be on disk at `from`"
        );
        assert_eq!(f.vfs.read(Path::new("/b.md")).expect("b"), b"ours");
    }

    /// The final unlink is disk hygiene, not data safety: it runs after
    /// both commits, so its failure downgrades to a leftover file.
    #[test]
    fn rename_replace_remove_failure_still_reports_replaced_with_the_blob_committed() {
        let mut f = fixture(b"ours");
        publish(&f.vfs, Path::new("/b.md"), b"theirs");
        let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");
        f.vfs.fail_next(VfsOp::Remove, io::ErrorKind::Other);

        let out = rename_replace(
            &mut f.conn,
            &f.vfs,
            f.ds,
            Path::new("/a.md"),
            Path::new("/b.md"),
            seen,
            SystemTime::now(),
        )
        .expect("a failed unlink must not fail the replace");

        let RenameOutcome::Replaced { displaced } = out else {
            panic!("expected Replaced, got {out:?}");
        };
        let blob = retry::with_retry(&mut f.conn, |tx| {
            crate::blob::get_blob(tx, &displaced.blob_hash)
        })
        .expect("blob committed");
        assert_eq!(blob, b"theirs");
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");
        assert_eq!(
            f.vfs.read(Path::new("/a.md")).expect("leftover"),
            b"theirs",
            "the leftover at `from` is Tolerable (§0.1 rung 3)"
        );
    }

    /// If the replaced file happened to have its own `documents` row, that
    /// row must stop claiming the path — two rows must never both claim one
    /// file (§1.7).
    #[test]
    fn rename_replace_blanks_the_displaced_documents_row_path() {
        let mut f = fixture(b"ours");
        publish(&f.vfs, Path::new("/b.md"), b"theirs");
        let other = seed_doc_with_path(&f.conn, "/b.md");
        let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");

        rename_replace(
            &mut f.conn,
            &f.vfs,
            f.ds,
            Path::new("/a.md"),
            Path::new("/b.md"),
            seen,
            SystemTime::now(),
        )
        .expect("rename_replace");

        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");
        assert_eq!(
            doc_path(&f.conn, other),
            "",
            "the displaced document's row must no longer claim /b.md"
        );
    }
}
