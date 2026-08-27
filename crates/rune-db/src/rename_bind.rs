//! The non-destructive rename entry point. Split out of `rename.rs`
//! — see that module's doc comment for the shared design.

use std::io;
use std::path::Path;
use std::time::SystemTime;

use rune_vfs::Vfs;

use crate::Error;
#[cfg(test)]
use crate::ids::DocId;
use crate::materialize::DocSession;
use crate::rename::{RenameOutcome, rebind};

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
/// longer exists and refuses with `MatResult::Missing` rather than writing
/// anywhere unexpected.
pub(crate) fn rename_bind(
    conn: &mut rusqlite::Connection,
    vfs: &dyn Vfs,
    ds: DocSession,
    from: &Path,
    to: &Path,
    now: SystemTime,
) -> Result<RenameOutcome, Error> {
    let durable = match vfs.rename_excl(from, to) {
        Ok(()) => true,
        Err(e) if rune_vfs::published_not_durable(&e) => false,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let seen = vfs.stat(to).map_err(Error::Io)?;
            return Ok(RenameOutcome::Collided { seen });
        }
        Err(e) => return Err(Error::Io(e)),
    };

    rebind(conn, vfs, ds, to, now)?;
    Ok(RenameOutcome::Renamed {
        to: to.to_path_buf(),
        durable,
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
    use std::path::PathBuf;

    use rune_vfs::{Mem, OpKind as VfsOp};
    use rusqlite::{Connection, params};

    use super::*;
    use crate::test_support::open;

    fn seed_doc_with_path(conn: &Connection, path: &str) -> DocId {
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES (?1, 'x', 'x')",
            params![path],
        )
        .expect("seed doc");
        DocId(conn.last_insert_rowid())
    }

    fn disk(vfs: &Mem, path: &Path) -> Vec<u8> {
        rune_vfs::get(vfs, path, rune_vfs::MAX_DOCUMENT_BYTES)
            .expect("disk content")
            .bytes
    }

    fn gone(vfs: &Mem, path: &Path) -> bool {
        rune_vfs::get(vfs, path, rune_vfs::MAX_DOCUMENT_BYTES).is_err()
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
                to: PathBuf::from("/b.md"),
                durable: true,
            }
        );
        assert_eq!(disk(&f.vfs, Path::new("/b.md")), b"hello");
        assert!(gone(&f.vfs, Path::new("/a.md")), "from must be gone");
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");

        let after = f.vfs.stat(Path::new("/b.md")).expect("stat after");
        assert_eq!(
            after.identity, before.identity,
            "rename preserves file identity (inode+device)"
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
        assert_eq!(disk(&f.vfs, Path::new("/a.md")), b"ours");
        assert_eq!(disk(&f.vfs, Path::new("/b.md")), b"theirs");
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/a.md");
        assert_eq!(obs_count(&f.conn), obs_before);
    }

    /// A `published_not_durable` `rename_excl` failure — the move physically
    /// took effect but its durability could not be confirmed — must still
    /// be reported as `Renamed`, not surfaced as an error: the row is
    /// rebound exactly as on a fully durable rename, and `durable` comes
    /// back `false`.
    #[test]
    fn rename_bind_reports_success_with_durable_false_on_an_unconfirmed_rename() {
        let mut f = fixture(b"hello");
        f.vfs.fail_after(VfsOp::RenameExcl, io::ErrorKind::Other);

        let out = rename_bind(
            &mut f.conn,
            &f.vfs,
            f.ds,
            Path::new("/a.md"),
            Path::new("/b.md"),
            SystemTime::now(),
        )
        .expect("an unconfirmed-durability publish must not fail the rename");

        assert_eq!(
            out,
            RenameOutcome::Renamed {
                to: PathBuf::from("/b.md"),
                durable: false,
            }
        );
        assert_eq!(disk(&f.vfs, Path::new("/b.md")), b"hello");
        assert!(gone(&f.vfs, Path::new("/a.md")), "from must be gone");
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");
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

        assert_eq!(disk(&f.vfs, Path::new("/a.md")), b"ours");
        assert!(gone(&f.vfs, Path::new("/b.md")));
        assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/a.md");
    }
}
