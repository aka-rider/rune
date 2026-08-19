//! The destructive `[R]eplace` rename entry point. Split out of
//! `rename.rs` — see that module's doc comment for the shared
//! design.

use std::io;
use std::path::Path;
use std::time::SystemTime;

use rune_vfs::{Stat, Vfs};

use crate::Error;
#[cfg(test)]
use crate::ids::DocId;
use crate::materialize::DocSession;
use crate::rename::{RenameOutcome, capture_and_rebind};

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
/// purpose: `schema.rs`'s `CHECK(origin IN (...))` is part of the frozen
/// on-disk vocabulary of the current schema version — adding a new value
/// is a schema-shape change, which ships as a new `rune-v{N}.db` rather
/// than an in-place bump (see `schema.rs`'s module doc). `'swap'`
/// already means "bytes an atomic `exchange` displaced", which is literally
/// the mechanism here.
pub(crate) fn rename_replace(
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

    // 2. The atomic publish. Both files still exist afterwards,
    //    with their contents swapped. `published_not_durable` means the
    //    swap physically took effect but its durability could not be
    //    confirmed — still a success, never a failure: the temp naming
    //    `from` still holds the displaced bytes, and step 3 reads them
    //    exactly as it would on a fully durable swap.
    let durable = match vfs.exchange(from, to) {
        Ok(()) => true,
        Err(e) if rune_vfs::published_not_durable(&e) => false,
        Err(e) => return Err(Error::Io(e)),
    };

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

    // 4. Capture before discard, physically, AND rebind — in ONE
    //    transaction (see the doc comment above): if this fails, NEITHER the
    //    observation nor the rebind took effect, our content is at `to`, the
    //    database still says `from` with its OLD identity, and `from` holds
    //    the foreign bytes untouched — a later ⌘S hashes those foreign
    //    bytes, mismatches `expect_obs`, and refuses. So `from` is
    //    deliberately NOT removed here.
    let displaced = capture_and_rebind(conn, vfs, ds, from, to, &displaced_bytes, now)?;

    // 5. The only lossy step in the design, strictly after the transaction
    //    committed. A failure here is disk hygiene, not data safety: the
    //    blob is already durable. Skipped when the swap's own durability is
    //    unconfirmed — `from` may still be the sole holder of the displaced
    //    bytes' physical copy, and must not be discarded.
    if durable {
        let _ = vfs.remove(from);
    }

    Ok(RenameOutcome::Replaced { displaced, durable })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use rune_vfs::{Mem, OpKind as VfsOp};
    use rusqlite::{Connection, params};

    use super::*;
    use crate::confirmation::Confirmation;
    use crate::obs_origin::ObsOrigin;
    use crate::observation::{self, ObservationMeta, StatFacts};
    use crate::retry;
    use crate::test_support::open;

    fn seed_doc_with_path(conn: &Connection, path: &str) -> DocId {
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES (?1, 'x', 'x')",
            params![path],
        )
        .expect("seed doc");
        DocId(conn.last_insert_rowid())
    }

    fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
        let temp = vfs.write_durable(path, bytes).expect("write_durable");
        vfs.rename_excl(&temp, path).expect("publish");
    }

    fn obs_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .expect("count observations")
    }

    fn doc_path(conn: &Connection, doc_id: DocId) -> String {
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

        let RenameOutcome::Replaced { displaced, .. } = out else {
            panic!("expected Replaced, got {out:?}");
        };
        assert_eq!(displaced.doc_id, f.ds.doc_id, "captured under OUR doc");
        assert_eq!(displaced.origin, ObsOrigin::Swap);
        assert_eq!(
            displaced.blob_hash.as_str(),
            observation::hash_bytes(b"theirs"),
            "the blob must be the REPLACED file's bytes"
        );

        let blob = retry::with_retry(&mut f.conn, |tx| {
            crate::blob::get_blob(tx, displaced.blob_hash.as_str())
        })
        .expect("displaced bytes durably stored");
        assert_eq!(blob, b"theirs");

        assert_eq!(f.vfs.read(Path::new("/b.md")).expect("read to"), b"ours");
        assert!(f.vfs.read(Path::new("/a.md")).is_err(), "from removed");
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");
    }

    /// Capture is unconditional — never gated on UTF-8 validity.
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

        let RenameOutcome::Replaced { displaced, .. } = out else {
            panic!("expected Replaced, got {out:?}");
        };
        let blob = retry::with_retry(&mut f.conn, |tx| {
            crate::blob::get_blob(tx, displaced.blob_hash.as_str())
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

    /// A `published_not_durable` `exchange` failure — the swap physically
    /// took effect but its durability could not be confirmed — must still
    /// be reported as `Replaced`, not surfaced as an error: the displaced
    /// bytes are captured into the blob store exactly as on a fully durable
    /// swap, `durable` comes back `false`, and `from` (the swap's own
    /// "temp", still holding the displaced bytes) is left on disk rather
    /// than unlinked.
    #[test]
    fn rename_replace_reports_success_with_durable_false_on_an_unconfirmed_exchange() {
        let mut f = fixture(b"ours");
        publish(&f.vfs, Path::new("/b.md"), b"theirs");
        let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");
        f.vfs.fail_after(VfsOp::Exchange, io::ErrorKind::Other);

        let out = rename_replace(
            &mut f.conn,
            &f.vfs,
            f.ds,
            Path::new("/a.md"),
            Path::new("/b.md"),
            seen,
            SystemTime::now(),
        )
        .expect("an unconfirmed-durability publish must not fail the replace");

        let RenameOutcome::Replaced { displaced, durable } = out else {
            panic!("expected Replaced, got {out:?}");
        };
        assert!(
            !durable,
            "unconfirmed exchange durability must report false"
        );

        let blob = retry::with_retry(&mut f.conn, |tx| {
            crate::blob::get_blob(tx, displaced.blob_hash.as_str())
        })
        .expect("displaced bytes durably stored despite unconfirmed exchange durability");
        assert_eq!(blob, b"theirs");

        assert_eq!(f.vfs.read(Path::new("/b.md")).expect("read to"), b"ours");
        assert_eq!(
            f.vfs.read(Path::new("/a.md")).expect("from kept"),
            b"theirs",
            "from must not be removed when the swap's own durability is unconfirmed"
        );
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");
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

        let RenameOutcome::Replaced { displaced, .. } = out else {
            panic!("expected Replaced, got {out:?}");
        };
        let blob = retry::with_retry(&mut f.conn, |tx| {
            crate::blob::get_blob(tx, displaced.blob_hash.as_str())
        })
        .expect("blob committed");
        assert_eq!(blob, b"theirs");
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");
        assert_eq!(
            f.vfs.read(Path::new("/a.md")).expect("leftover"),
            b"theirs",
            "a leftover at `from` is recoverable clutter, never data loss"
        );
    }

    /// If the replaced file happened to have its own `documents` row, that
    /// row must stop claiming the path — two rows must never both claim one
    /// file.
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

    /// The recorded risk this test pins down: a `rename_replace` displacing
    /// a bound document's file must never disturb THAT document's own merge
    /// ancestor. `other` (bound to `/b.md`, the destination) already has an
    /// ancestor-eligible observation of its own before the replace; the
    /// swap-captured displaced-bytes observation the replace records must
    /// not become — or replace — that ancestor.
    #[test]
    fn rename_replace_over_a_bound_document_leaves_the_displaced_documents_ancestor_unchanged() {
        let mut f = fixture(b"ours");
        publish(&f.vfs, Path::new("/b.md"), b"theirs");
        let other = seed_doc_with_path(&f.conn, "/b.md");
        let other_session = crate::session::establish_session(&f.conn, SystemTime::now())
            .expect("establish other session");

        // Give `other` its own ancestor-eligible observation — the merge
        // ancestor a later 3-way merge on `other` would use.
        retry::with_retry(&mut f.conn, |tx| {
            let blob_hash = crate::blob::put_blob(tx, b"theirs")?;
            observation::record_observation(
                tx,
                other,
                other_session,
                ObservationMeta {
                    blob_hash: &blob_hash,
                    seq: Some(0),
                    origin: ObsOrigin::Load,
                    confirmed: Confirmation::Unclassified,
                },
                &StatFacts {
                    size: Some(6),
                    mtime: Some("t".to_string()),
                    ..Default::default()
                },
                "t",
            )
        })
        .expect("seed other's ancestor observation");

        let before = retry::with_retry(&mut f.conn, |tx| {
            observation::ancestor_at(tx, other, other_session, 0, None)
        })
        .expect("ancestor before")
        .expect("other must have an ancestor before the replace");

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

        let after = retry::with_retry(&mut f.conn, |tx| {
            observation::ancestor_at(tx, other, other_session, 0, None)
        })
        .expect("ancestor after")
        .expect("other must still have an ancestor after the replace");

        assert_eq!(
            after, before,
            "the displaced document's merge ancestor must be byte-identical \
             after a rename_replace swapped its path away"
        );
    }
}
