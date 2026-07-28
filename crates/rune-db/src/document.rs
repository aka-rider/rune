//! Document identity resolution — ports Go's `OpenPath`. Only the slice WP4's `Load`
//! actually needs: resolving a real on-disk path to a stable `documents.id`,
//! preferring inode/device identity and falling back to path-keying when no
//! usable identity is available (§1.4.6). `Bind`/`DeleteDoc`/
//! `CreateScratch`/chat-doc reservation are workspace-level document
//! lifecycle operations outside WP4's Load/Probe/Materialize/adopt/reaper
//! scope (`Materialize`'s own re-Bind is inlined in `materialize::commit_save`,
//! matching Go's `commitSave`, which does not call `Bind` either) — left for
//! whichever later work package wires document creation/rename UI flows.
//!
//! Both branches below run the whole decide-then-write sequence inside ONE
//! `retry::with_retry` transaction (mirroring Go's `_txlock=immediate`-begun
//! `tx` in `openPathByName`/`openPathByInode`), closing the same
//! same-process-instances TOCTOU race Go's doc comment describes.

use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use rune_vfs::Vfs;

use crate::Error;
use crate::retry;

/// The stable document identity `OpenPath` resolves to, plus whether the
/// path arrived at a different name than the row already on file (a
/// detected rename, §1.4.6). Port of `store_documents.go`'s `DocRef`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocRef {
    pub id: i64,
    pub renamed_from: Option<String>,
}

/// Real (inode, device) identity for `path` via `vfs`, or `None` when the
/// stat failed or exposed no usable identity (`ino == 0`, matching Go's
/// `!ok || inode == 0` fallback-to-name-keying guard, `store_documents.go:19`).
fn stat_id(vfs: &dyn Vfs, path: &Path) -> Option<(i64, i64)> {
    let stat = vfs.stat(path).ok()?;
    match (stat.identity.inode, stat.identity.device) {
        (Some(inode), Some(device)) if inode != 0 => Some((inode as i64, device as i64)),
        _ => None,
    }
}

/// Resolves the VFS document for a file that exists on disk. Must only be
/// called after the file has been successfully read (so stat can obtain a
/// real inode) — `load::load` satisfies this by reading before calling.
/// Port of `store_documents.go:15-23` (`OpenPath`).
pub fn open_path(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    path: &Path,
    now: SystemTime,
) -> Result<DocRef, Error> {
    let at = crate::session::format_rfc3339_nanos(now);
    let path_str = path.to_string_lossy().into_owned();

    match stat_id(vfs, path) {
        None => retry::with_retry(conn, |tx| open_path_by_name(tx, &path_str, &at)),
        Some((inode, device)) => retry::with_retry(conn, |tx| {
            open_path_by_inode(tx, &path_str, inode, device, &at)
        }),
    }
}

/// Port of `store_documents.go:37-82` (`openPathByName`).
fn open_path_by_name(tx: &Transaction<'_>, path: &str, at: &str) -> Result<DocRef, Error> {
    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM documents WHERE path=?1 AND inode IS NULL",
            params![path],
            |r| r.get(0),
        )
        .optional()?;

    match existing {
        None => {
            tx.execute(
                "INSERT INTO documents(path, kind, created_at, last_seen_at) VALUES(?1,'file',?2,?2)",
                params![path, at],
            )?;
            Ok(DocRef {
                id: tx.last_insert_rowid(),
                renamed_from: None,
            })
        }
        Some(id) => {
            tx.execute(
                "UPDATE documents SET last_seen_at=?1 WHERE id=?2",
                params![at, id],
            )?;
            Ok(DocRef {
                id,
                renamed_from: None,
            })
        }
    }
}

/// Port of `store_documents.go:89-163` (`openPathByInode`).
fn open_path_by_inode(
    tx: &Transaction<'_>,
    path: &str,
    inode: i64,
    device: i64,
    at: &str,
) -> Result<DocRef, Error> {
    let existing: Option<(i64, String)> = tx
        .query_row(
            "SELECT id, path FROM documents WHERE inode=?1 AND device=?2",
            params![inode, device],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    match existing {
        None => {
            tx.execute(
                "UPDATE documents SET path='' WHERE path=?1 AND (inode IS NULL OR inode!=?2)",
                params![path, inode],
            )?;
            tx.execute(
                "INSERT INTO documents(path, inode, device, kind, created_at, last_seen_at) \
                 VALUES(?1,?2,?3,'file',?4,?4)",
                params![path, inode, device, at],
            )?;
            Ok(DocRef {
                id: tx.last_insert_rowid(),
                renamed_from: None,
            })
        }
        Some((row_id, row_path)) => {
            if row_path != path {
                tx.execute(
                    "UPDATE documents SET path='' WHERE path=?1 AND id!=?2",
                    params![path, row_id],
                )?;
                tx.execute(
                    "UPDATE documents SET path=?1, inode=?2, device=?3, last_seen_at=?4 WHERE id=?5",
                    params![path, inode, device, at, row_id],
                )?;
                Ok(DocRef {
                    id: row_id,
                    renamed_from: Some(row_path),
                })
            } else {
                tx.execute(
                    "UPDATE documents SET last_seen_at=?1 WHERE id=?2",
                    params![at, row_id],
                )?;
                Ok(DocRef {
                    id: row_id,
                    renamed_from: None,
                })
            }
        }
    }
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

    #[test]
    fn open_path_by_name_reuses_the_same_row_on_a_second_open() {
        let mut conn = open();
        let vfs = Mem::new(); // no file present -> stat_id is None -> name-keyed path
        let path = Path::new("/does/not/exist.md");

        let first = open_path(&mut conn, &vfs, path, SystemTime::now()).expect("first open");
        let second = open_path(&mut conn, &vfs, path, SystemTime::now()).expect("second open");
        assert_eq!(first.id, second.id, "name-keyed reopen must reuse the row");
        assert!(second.renamed_from.is_none());
    }

    #[test]
    fn open_path_by_inode_detects_a_rename() {
        let mut conn = open();
        let vfs = Mem::new();
        let old_path = Path::new("/doc/old.md");
        let new_path = Path::new("/doc/new.md");

        // Publish a real file at old_path (write_durable's sibling temp,
        // then rename_excl onto old_path — the same shape materialize uses).
        let temp = vfs.write_durable(old_path, b"hello").expect("temp");
        vfs.rename_excl(&temp, old_path).expect("publish");

        let first = open_path(&mut conn, &vfs, old_path, SystemTime::now()).expect("first open");

        // `Mem::rename_excl` moves the same file object (inode travels with
        // it) from old_path to new_path — a genuine on-disk rename.
        vfs.rename_excl(old_path, new_path)
            .expect("simulate rename");

        let second = open_path(&mut conn, &vfs, new_path, SystemTime::now()).expect("second open");
        assert_eq!(second.id, first.id, "same inode, new path -> same doc id");
        assert_eq!(second.renamed_from.as_deref(), Some("/doc/old.md"));
    }
}
