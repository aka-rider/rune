//! The document-row rebind/identity primitives — pointing a `documents`
//! row at a real on-disk path+identity, and the eviction that keeps
//! "two rows must never both claim the same file" true while doing
//! it. Split out of `materialize.rs`; shared by `materialize.rs`
//! itself, `rename.rs`, and `document.rs`.

use rusqlite::params;

use crate::Error;
use crate::doc_kind::DocKind;
use crate::ids::DocId;
use crate::observation;

/// The path/identity half of a document rebind, bundled for the same
/// argument-count reason as `materialize::DocSession`.
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
/// (inode, device) — one-value-one-meaning applied to the
/// path/identity columns: two rows must never both claim the same file.
///
/// A rename must NOT go through `materialize::commit_save_from_stat`: that
/// also does `put_blob(facts.data)` + `record_adoption_tx(origin='save')`,
/// which after renaming a *dirty* document would move `saved_obs` to an
/// observation claiming the disk holds the journal head. The next ⌘S would
/// then CAS against a lie.
///
/// Caller-supplied transaction: this is pure SQLite with no `vfs` call
/// inside, so it is safe to run under an open tx (invariant I1).
pub(crate) fn rebind_document_tx(
    tx: &rusqlite::Connection,
    doc_id: DocId,
    rebind: Rebind<'_>,
) -> Result<(), Error> {
    let stat = rebind.stat;

    evict_path_claim_tx(tx, rebind.path, doc_id)?;

    if stat.inode.is_some() {
        tx.execute(
            "UPDATE documents SET inode=NULL, device=NULL WHERE inode=?1 AND device=?2 AND id!=?3",
            params![stat.inode, stat.device, doc_id],
        )?;
    }

    set_identity_tx(tx, doc_id, rebind.path, stat.inode, stat.device, rebind.at)
}

/// Evicts any OTHER row currently claiming `path` — "two rows must
/// never both claim the same file" applied to the `path` column, everywhere
/// a document row is about to be pointed at a real path. The ONE eviction
/// chokepoint for this exact statement: [`rebind_document_tx`] and
/// `document::open_path_by_inode`'s rename-detected branch both route
/// through this instead of each carrying their own copy — the two copies
/// had already drifted once before this extraction ([rune-db 13]).
pub(crate) fn evict_path_claim_tx(
    tx: &rusqlite::Connection,
    path: &str,
    keep_id: DocId,
) -> Result<(), Error> {
    tx.execute(
        "UPDATE documents SET path='' WHERE path=?1 AND id!=?2",
        params![path, keep_id],
    )?;
    Ok(())
}

/// Points `doc_id`'s row at `path`'s real on-disk identity — the ONE "set
/// this row's path/inode/device to a real file's identity" statement.
/// Always sets `kind='file'`: every caller (a post-publish rebind, or
/// `document::open_path_by_inode` discovering a path change) only ever
/// calls this once a real, on-disk file is confirmed.
pub(crate) fn set_identity_tx(
    tx: &rusqlite::Connection,
    doc_id: DocId,
    path: &str,
    inode: Option<i64>,
    device: Option<i64>,
    at: &str,
) -> Result<(), Error> {
    tx.execute(
        "UPDATE documents SET path=?1, inode=?2, device=?3, kind=?4, last_seen_at=?5 WHERE id=?6",
        params![path, inode, device, DocKind::File.as_str(), at, doc_id],
    )?;
    Ok(())
}
