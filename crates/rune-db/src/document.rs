//! Document identity resolution. Only the slice WP4's `Load`
//! actually needs: resolving a real on-disk path to a stable `documents.id`,
//! preferring inode/device identity and falling back to path-keying when no
//! usable identity is available. `Bind`/`DeleteDoc`/
//! `CreateScratch`/chat-doc reservation are workspace-level document
//! lifecycle operations outside WP4's Load/Probe/Materialize/adopt/reaper
//! scope (`materialize::commit_save` inlines its own re-Bind rather than
//! calling `Bind`) — left for whichever later work package wires document
//! creation/rename UI flows.
//!
//! Both branches below run the whole decide-then-write sequence inside ONE
//! `retry::with_retry` transaction, closing the same-process-instances
//! TOCTOU race between the identity read and the write it drives.

use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use rune_vfs::Vfs;

use crate::Error;
use crate::doc_kind::DocKind;
use crate::ids::DocId;
use crate::retry;

/// The stable document identity `open_path` resolves to, plus whether the
/// path arrived at a different name than the row already on file (a
/// detected rename).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocRef {
    pub id: DocId,
    pub renamed_from: Option<String>,
}

/// Real (inode, device) identity for `path` via `vfs`, or `None` when the
/// stat failed or exposed no usable identity (`ino == 0` triggers the
/// fallback-to-name-keying guard).
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
pub fn open_path(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    path: &Path,
    now: SystemTime,
) -> Result<DocRef, Error> {
    let at = crate::session::format_rfc3339_nanos(now);
    let path_str = crate::paths::to_db_string(path)?;

    match stat_id(vfs, path) {
        None => retry::with_retry(conn, |tx| open_path_by_name(tx, &path_str, &at)),
        Some((inode, device)) => retry::with_retry(conn, |tx| {
            open_path_by_inode(tx, &path_str, inode, device, &at)
        }),
    }
}

fn open_path_by_name(tx: &Transaction<'_>, path: &str, at: &str) -> Result<DocRef, Error> {
    let existing: Option<DocId> = tx
        .query_row(
            "SELECT id FROM documents WHERE path=?1 AND inode IS NULL",
            params![path],
            |r| r.get(0),
        )
        .optional()?;

    match existing {
        None => {
            tx.execute(
                "INSERT INTO documents(path, kind, created_at, last_seen_at) VALUES(?1,?2,?3,?3)",
                params![path, DocKind::File.as_str(), at],
            )?;
            Ok(DocRef {
                id: DocId(tx.last_insert_rowid()),
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

fn open_path_by_inode(
    tx: &Transaction<'_>,
    path: &str,
    inode: i64,
    device: i64,
    at: &str,
) -> Result<DocRef, Error> {
    let existing: Option<(DocId, String)> = tx
        .query_row(
            "SELECT id, path FROM documents WHERE inode=?1 AND device=?2",
            params![inode, device],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    match existing {
        None => {
            // No row claims this exact (inode, device) — but a row may
            // still claim `path` under a DIFFERENT (or no) identity, e.g.
            // an external atomic-swap overwrite: the writer's rename onto
            // `path` mints a brand-new inode, but it is still the SAME
            // document as far as the user is concerned. Reclaiming that
            // row's identity keeps its journal intact instead of orphaning
            // it behind a freshly minted, journal-empty row. Only when
            // nothing at all claims `path` is a fresh row actually minted.
            //
            // A hardlinked sibling that survives this reclaim under its OWN
            // path is a known, accepted edge: the reclaimed row's identity
            // moves to the new inode, so a later open of the hardlink's own
            // path resolves as a distinct document rather than the one
            // being tracked here.
            // A unique partial index enforces that at most one row can ever
            // claim a given non-empty path, so there is no tie to break
            // here.
            let claimant: Option<DocId> = tx
                .query_row(
                    "SELECT id FROM documents WHERE path=?1",
                    params![path],
                    |r| r.get(0),
                )
                .optional()?;

            if let Some(row_id) = claimant {
                crate::rebind::set_identity_tx(tx, row_id, path, Some(inode), Some(device), at)?;
                return Ok(DocRef {
                    id: row_id,
                    renamed_from: None,
                });
            }

            tx.execute(
                "INSERT INTO documents(path, inode, device, kind, created_at, last_seen_at) \
                 VALUES(?1,?2,?3,?4,?5,?5)",
                params![path, inode, device, DocKind::File.as_str(), at],
            )?;
            Ok(DocRef {
                id: DocId(tx.last_insert_rowid()),
                renamed_from: None,
            })
        }
        Some((row_id, row_path)) => {
            if row_path != path {
                // Both statements route through `rebind`'s eviction/
                // rebind chokepoints — the same two this module's own
                // divergent copies used to drift from ([rune-db 13]).
                crate::rebind::evict_path_claim_tx(tx, path, row_id)?;
                crate::rebind::set_identity_tx(tx, row_id, path, Some(inode), Some(device), at)?;
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

/// The `limit` most recently opened real-file document paths, newest
/// first — the fuzzy file finder's own MRU list. `last_seen_at` bumps on
/// every `open_path` call above, and its fixed-width RFC3339-nanos text
/// sorts lexicographically into MRU order, the same shape `search_history::
/// recent` already relies on for its own MRU column. An evicted row
/// (`path=''`) and a non-`'file'` `kind` (scratch/chat) are excluded: only
/// a still-named, real file belongs in the finder's list.
pub fn recent_paths(conn: &Connection, limit: u32) -> Result<Vec<String>, Error> {
    let mut stmt = conn.prepare(&format!(
        "SELECT path FROM documents WHERE path != '' AND kind = '{}' \
         ORDER BY last_seen_at DESC LIMIT ?1",
        DocKind::File.as_str()
    ))?;
    let rows = stmt.query_map(params![limit], |r| r.get::<_, String>(0))?;
    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_vfs::Mem;
    use std::time::Duration;

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

    /// B3 (data-loss fix): an external ATOMIC SWAP overwrite mints a NEW
    /// inode at the SAME path — this must reclaim the existing row's
    /// identity, never orphan it behind a freshly minted, journal-empty
    /// row. Port of a real bug: the prior eviction here (`UPDATE documents
    /// SET path=''`) is what silently dropped a dead session's draft on
    /// reopen after an external edit.
    #[test]
    fn open_path_by_inode_reclaims_the_row_after_an_external_atomic_swap() {
        let mut conn = open();
        let vfs = Mem::new();
        let path = Path::new("/doc.md");

        let temp = vfs.write_durable(path, b"hello").expect("temp");
        vfs.rename_excl(&temp, path).expect("publish");
        let first = open_path(&mut conn, &vfs, path, SystemTime::now()).expect("first open");

        // An external atomic-swap overwrite at the SAME path — a genuinely
        // new inode, unlike a rename (which moves the same inode).
        vfs.save_atomic(path, b"swapped externally")
            .expect("external atomic swap");

        let second = open_path(&mut conn, &vfs, path, SystemTime::now()).expect("second open");
        assert_eq!(
            second.id, first.id,
            "the swap must reclaim the existing row, not mint a new one"
        );
        assert!(
            second.renamed_from.is_none(),
            "same path -> not a detected rename"
        );

        let row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .expect("count");
        assert_eq!(
            row_count, 1,
            "no orphaned row should be left behind by the swap"
        );
    }

    /// A4/[rune-db 6]: a path that doesn't round-trip through UTF-8 must be
    /// rejected loudly at bind — never silently mangled into a `documents.path`
    /// TEXT column via `to_string_lossy`. Exercised through the name-keyed
    /// branch (no real file present, so `stat_id` is `None`) since that is
    /// the branch every never-yet-seen document path binds through first.
    #[test]
    fn open_path_rejects_a_non_utf8_path_at_bind() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let mut conn = open();
        let vfs = Mem::new();
        let bytes: &[u8] = &[0x2f, 0xff, 0xfe, 0x2e, 0x6d, 0x64]; // "/\xFF\xFE.md"
        let path = Path::new(OsStr::from_bytes(bytes));

        let err = open_path(&mut conn, &vfs, path, SystemTime::now())
            .expect_err("a non-utf8 path must be refused, not silently mangled");
        assert!(matches!(err, Error::Invalid(_)));
    }

    #[test]
    fn recent_paths_returns_reverse_open_order_excluding_evicted_and_scratch() {
        let mut conn = open();
        let vfs = Mem::new();
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);

        open_path(&mut conn, &vfs, Path::new("/doc/a.md"), base).expect("open a");
        open_path(
            &mut conn,
            &vfs,
            Path::new("/doc/b.md"),
            base + Duration::from_secs(10),
        )
        .expect("open b");
        open_path(
            &mut conn,
            &vfs,
            Path::new("/doc/c.md"),
            base + Duration::from_secs(20),
        )
        .expect("open c");

        // An evicted row (`path=''`) and a scratch-kind row must never
        // surface, even though both sort newest by `last_seen_at`.
        conn.execute(
            "INSERT INTO documents(path, kind, created_at, last_seen_at) \
             VALUES('', 'file', '2000', '9999')",
            [],
        )
        .expect("insert evicted row");
        conn.execute(
            "INSERT INTO documents(path, kind, created_at, last_seen_at) \
             VALUES('/doc/scratch.md', 'scratch', '2000', '9998')",
            [],
        )
        .expect("insert scratch row");

        let recent = recent_paths(&conn, 10).expect("recent_paths");
        assert_eq!(
            recent,
            vec![
                "/doc/c.md".to_string(),
                "/doc/b.md".to_string(),
                "/doc/a.md".to_string(),
            ]
        );
    }
}
