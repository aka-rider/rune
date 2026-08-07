//! Rename — moving a bound document from one path to another, and the
//! destructive `[R]eplace` variant that renames *onto* a file that already
//! exists.
//!
//! Two entry points, each a single writer op that is **never split across a
//! message boundary** — [`rename_bind`](crate::rename_bind::rename_bind)
//! (the non-destructive case, `rename_bind.rs`) and
//! [`rename_replace`](crate::rename_replace::rename_replace) (the
//! confirmed-destructive case, `rename_replace.rs`). This module holds what
//! both share: the outcome type and the two transaction primitives beneath
//! them.
//!
//! - **rename_bind** — `renamex_np(RENAME_EXCL)` (a no-clobber
//!   atomic publish). If the destination exists it returns
//!   [`RenameOutcome::Collided`] having written *nothing*, and the UI
//!   raises a guard before a destructive transition.
//! - **rename_replace** — capture-then-swap-then-commit-then-unlink, in
//!   that order, so "capture before discard — physically" holds
//!   by mechanism: the replaced file's bytes are a durable blob before its
//!   last name is unlinked. That blob is the only record the replaced file
//!   ever existed, since rune never opened it.
//!
//! Why the two halves are ONE op each: the capture and the swap cannot be
//! separated by a message round-trip without making "swapped but not
//! captured" a representable state.
//!
//! ### What a rename deliberately does NOT do
//!
//! - **No save.** The only two acts that touch the destination are ⌘S and
//!   save-on-close; a rename is neither. A dirty document renames
//!   and stays dirty — history is keyed to inode+device and `renamex_np`
//!   preserves the inode, so no history is orphaned.
//! - **No observation for a plain rename.** `observations` (`schema.rs`)
//!   has no path column: it records blob_hash/size/mtime/inode/device/seq,
//!   none of which `renamex_np` changes. The existing `saved_obs` stays
//!   exactly as valid, and a spurious new one would move the CAS baseline.
//! - **No `commit_save`.** It would `put_blob(buffer)` +
//!   `record_adoption_tx(origin='save')`, i.e. claim the disk holds the
//!   journal head. After renaming a *dirty* document the next ⌘S would then
//!   CAS against a lie. Only `rebind_document_tx` is reused.
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

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rune_vfs::{Stat, Vfs};

use crate::Error;
use crate::materialize::DocSession;
use crate::observation::{self, Observation, ObserveInput};
use crate::rebind::{Rebind, rebind_document_tx};
use crate::retry;

/// The outcome of `rename_bind` / `rename_replace`.
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
    /// and that `rename_replace` re-checks.
    Collided { seen: Stat },
    /// The replace committed. `displaced` is the observation of the bytes
    /// that used to live at the destination, already durably captured as a
    /// blob — `displaced.blob_hash` retrieves them via `get_blob`.
    Replaced { displaced: Observation },
    /// The destination no longer matches the `seen` the user consented to,
    /// so the replace was abandoned before touching anything. `fresh` is
    /// what the destination looks like now.
    Refused { fresh: Stat },
}

/// The step-4 primitive `rename_replace` calls: puts `displaced_bytes` as a
/// blob, records the `origin='swap'` observation of them (captured at
/// `from`, where the displaced file object now lives), and rebinds the
/// document row to `to` — all inside ONE transaction, closing the crash
/// window [rune-db 4] describes. Both stats (disk I/O) run BEFORE the
/// transaction opens (invariant I1); the transaction itself is pure SQLite.
///
/// Audited invariant: the displaced-bytes observation can never become the
/// merge ancestor for the document that used to be bound to `to`, let alone
/// for any other document. Two facts hold this shut independently. First,
/// this observation is recorded under the RENAMING document's own id, never
/// under the id of whichever row used to claim `to` — ancestor selection is
/// scoped by document id, so an observation captured here can never be
/// selected as a *different* document's ancestor even in principle. Second,
/// it is unconditionally recorded with `origin='swap'`, and ancestor
/// selection only considers `origin IN ('load','save','resolve')` — so it is
/// never ancestor-eligible for ANY document, including the renaming one.
pub(crate) fn capture_and_rebind(
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
                confirmed: None,
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
pub(crate) fn rebind(
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
