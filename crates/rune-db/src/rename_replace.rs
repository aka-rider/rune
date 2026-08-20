//! The destructive `[R]eplace` rename entry point. Split out of
//! `rename.rs` — see that module's doc comment for the shared
//! design.

use std::io;
use std::path::Path;
use std::time::SystemTime;

use rune_vfs::{Stat, Vfs};

use crate::Error;
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
#[path = "rename_replace_tests.rs"]
mod tests;
