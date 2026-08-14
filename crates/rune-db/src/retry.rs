//! SQLite busy/contention retry classifier and the `BEGIN IMMEDIATE`
//! chokepoint every writer-thread op runs through (plan Gotchas).
//!
//! rusqlite already sets `busy_timeout = 5000ms` on open and enables
//! extended result codes (`SQLITE_OPEN_EXRESCODE`) by default. Two distinct
//! situations follow a busy connection:
//!
//! - **Extended code 517 (`SQLITE_BUSY_SNAPSHOT`)**: this transaction's
//!   snapshot is stale — a `DEFERRED` read that later tried to upgrade to a
//!   write found the database changed underneath it. The busy handler is
//!   **not** invoked for it (retrying the same statement cannot help); the
//!   only correct response is to roll back and restart the **whole**
//!   transaction from the top so it re-reads a fresh snapshot. This is
//!   exactly why every writer-thread op begins with `BEGIN IMMEDIATE`
//!   (`transaction_with_behavior(TransactionBehavior::Immediate)`) instead
//!   of the default `DEFERRED` — taking the write lock immediately is what
//!   makes 517 unreachable in the steady state; it can still occur under
//!   real contention from another process, which is exactly the case this
//!   classifier exists for.
//! - **Primary code 5 (`SQLITE_BUSY`)**: the busy handler already waited up
//!   to `busy_timeout` and the lock still didn't clear. One more read/write
//!   attempt at the same statement can still succeed (the lock holder may
//!   release between our own timeout and the next try), so this classifies
//!   as a jittered backoff, capped at a small number of attempts, rather
//!   than an immediate whole-transaction restart.
//!
//! Anything else surfaces as-is — constraint violations, corruption, and
//! the like are never retried.

use std::time::Duration;

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::Error;

/// How `retry::classify` interprets a failed op.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Classification {
    /// Extended code 517 (`SQLITE_BUSY_SNAPSHOT`): roll back and restart the
    /// whole transaction.
    RestartTransaction,
    /// Primary code 5 (`SQLITE_BUSY`) after the busy handler already
    /// waited: jittered backoff, then retry the same transaction attempt.
    Backoff,
    /// Anything else: surface immediately, no retry.
    Surface,
}

/// Classifies a failed SQLite operation per the module doc.
pub(crate) fn classify(err: &rusqlite::Error) -> Classification {
    const SQLITE_BUSY_SNAPSHOT: i32 = 517;

    if err.sqlite_extended_error_code() == Some(SQLITE_BUSY_SNAPSHOT) {
        return Classification::RestartTransaction;
    }
    if err.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy) {
        return Classification::Backoff;
    }
    Classification::Surface
}

/// `RestartTransaction` has no cap in the plan's Gotchas (517 unconditionally
/// restarts) — a generous defensive ceiling avoids a pathological infinite
/// loop under sustained cross-process contention without changing observed
/// behavior in any realistic scenario. `Backoff` is explicitly capped at 5
/// (plan Gotchas: "jittered backoff ≤5 attempts").
const MAX_RESTART_ATTEMPTS: u32 = 20;
const MAX_BACKOFF_ATTEMPTS: u32 = 5;

/// Runs `op` inside `BEGIN IMMEDIATE`, retrying per [`classify`] on failure,
/// and commits on success. `op` must never touch the filesystem via
/// `rune-vfs` — no DB transaction is ever held across a `vfs` call (plan
/// binding rule, invariant I1).
///
/// The classifier covers the WHOLE lifecycle of an attempt — acquiring the
/// write lock (`BEGIN IMMEDIATE` itself can surface BUSY under real
/// multiprocess contention, the single most common contention point) and
/// `COMMIT`, not just `op`'s own body. A busy/snapshot-stale failure at
/// either of those points is exactly as retryable as one from `op` — never
/// a hard error.
pub fn with_retry<T>(
    conn: &mut Connection,
    mut op: impl FnMut(&Transaction) -> Result<T, Error>,
) -> Result<T, Error> {
    let mut restart_attempts = 0u32;
    let mut backoff_attempts = 0u32;

    loop {
        let tx = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(e) => match step(&e, &mut restart_attempts, &mut backoff_attempts) {
                Step::Retry => continue,
                Step::Surface => return Err(Error::from(e)),
            },
        };
        match op(&tx) {
            Ok(value) => match tx.commit() {
                Ok(()) => return Ok(value),
                Err(e) => match step(&e, &mut restart_attempts, &mut backoff_attempts) {
                    Step::Retry => continue,
                    Step::Surface => return Err(Error::from(e)),
                },
            },
            Err(err) => {
                // `Transaction::drop` would roll back anyway if not
                // committed; the explicit call just does it eagerly. A
                // failure here leaves nothing uncommitted that wasn't
                // already going to be discarded — `err` below is what
                // actually gets surfaced.
                let _ = tx.rollback();
                let sqlite_err = match &err {
                    Error::Sqlite(e) => Some(e),
                    Error::Io(_)
                    | Error::WriterQueueFull
                    | Error::WriterGone
                    | Error::ReaderGone
                    | Error::SessionEstablish(_)
                    | Error::WalModeUnavailable(_)
                    | Error::CorruptPayload(_)
                    | Error::BlobHashMismatch { .. }
                    | Error::ReplayFailed(_)
                    | Error::NotFound(_)
                    | Error::Invalid(_) => None,
                };
                let Some(sqlite_err) = sqlite_err else {
                    return Err(err);
                };
                match step(sqlite_err, &mut restart_attempts, &mut backoff_attempts) {
                    Step::Retry => continue,
                    Step::Surface => return Err(err),
                }
            }
        }
    }
}

/// What [`with_retry`] does next after classifying a failed SQLite call —
/// `Retry` also performs the jittered sleep for the `Backoff` case (a
/// caller that gets `Retry` back need only `continue` the loop), so the
/// three call sites in [`with_retry`] (acquire/op/commit) share one
/// decision instead of duplicating the counter/cap bookkeeping three times.
enum Step {
    Retry,
    Surface,
}

fn step(err: &rusqlite::Error, restart_attempts: &mut u32, backoff_attempts: &mut u32) -> Step {
    match classify(err) {
        Classification::RestartTransaction => {
            *restart_attempts += 1;
            if *restart_attempts > MAX_RESTART_ATTEMPTS {
                return Step::Surface;
            }
            Step::Retry
        }
        Classification::Backoff => {
            *backoff_attempts += 1;
            if *backoff_attempts > MAX_BACKOFF_ATTEMPTS {
                return Step::Surface;
            }
            std::thread::sleep(jittered_backoff(*backoff_attempts));
            Step::Retry
        }
        Classification::Surface => Step::Surface,
    }
}

/// Per-attempt growth of [`jittered_backoff`]'s base delay.
const BACKOFF_STEP_MS: u64 = 5;
/// Width of the jitter [`jittered_backoff`] adds on top of the base delay
/// (derived from this process's own pid, so multiple contending processes
/// don't retry in lockstep).
const BACKOFF_JITTER_WIDTH_MS: u64 = 8;

/// A short, monotonically-growing backoff with a little jitter so multiple
/// contending connections don't retry in lockstep. Not seeded from an
/// injectable clock (unlike the coalescing clock elsewhere in this crate):
/// this delay paces real cross-process lock contention, not something a
/// test should ever need to control deterministically — the same way
/// rusqlite's own `busy_timeout` isn't either.
fn jittered_backoff(attempt: u32) -> Duration {
    let base_ms = BACKOFF_STEP_MS * u64::from(attempt);
    let jitter_ms = u64::from(std::process::id()) % BACKOFF_JITTER_WIDTH_MS;
    Duration::from_millis(base_ms + jitter_ms)
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
    use rusqlite::ffi;

    fn busy(extended_code: i32) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(ffi::Error::new(extended_code), None)
    }

    #[test]
    fn classifies_517_as_restart_transaction() {
        assert_eq!(classify(&busy(517)), Classification::RestartTransaction);
    }

    #[test]
    fn classifies_primary_5_as_backoff() {
        assert_eq!(classify(&busy(5)), Classification::Backoff);
    }

    #[test]
    fn classifies_locked_as_surface() {
        // SQLITE_LOCKED (6): a different code family entirely, never
        // retried by this classifier.
        assert_eq!(classify(&busy(6)), Classification::Surface);
    }

    #[test]
    fn classifies_constraint_violation_as_surface() {
        // SQLITE_CONSTRAINT (19): never transient, never retried.
        assert_eq!(classify(&busy(19)), Classification::Surface);
    }

    /// [`step`] is the ONE decision point `with_retry` now routes ALL three
    /// call sites through (acquire/op/commit) — this proves its retry/cap/
    /// surface behavior directly, independent of which call site feeds it,
    /// covering the acquisition- and commit-path restructuring (finding 4)
    /// without needing a real, timing-sensitive multiprocess BUSY.
    #[test]
    fn step_retries_517_up_to_the_restart_cap_then_surfaces() {
        let mut restart = 0u32;
        let mut backoff = 0u32;
        for _ in 0..MAX_RESTART_ATTEMPTS {
            assert!(matches!(
                step(&busy(517), &mut restart, &mut backoff),
                Step::Retry
            ));
        }
        assert!(matches!(
            step(&busy(517), &mut restart, &mut backoff),
            Step::Surface
        ));
        assert_eq!(backoff, 0, "517 must never touch the backoff counter");
    }

    #[test]
    fn step_retries_primary_5_up_to_the_backoff_cap_then_surfaces() {
        let mut restart = 0u32;
        let mut backoff = 0u32;
        for _ in 0..MAX_BACKOFF_ATTEMPTS {
            assert!(matches!(
                step(&busy(5), &mut restart, &mut backoff),
                Step::Retry
            ));
        }
        assert!(matches!(
            step(&busy(5), &mut restart, &mut backoff),
            Step::Surface
        ));
        assert_eq!(restart, 0, "primary 5 must never touch the restart counter");
    }

    #[test]
    fn step_surfaces_a_non_retryable_error_immediately() {
        let mut restart = 0u32;
        let mut backoff = 0u32;
        assert!(matches!(
            step(&busy(19), &mut restart, &mut backoff),
            Step::Surface
        ));
        assert_eq!(restart, 0);
        assert_eq!(backoff, 0);
    }

    /// Acquisition failures now enter the SAME retry loop as an `op`
    /// failure: with a write lock held open on a SEPARATE connection to the
    /// same file, `with_retry`'s own `transaction_with_behavior` on this
    /// connection must eventually succeed once the lock is released, never
    /// surface a hard error just because the FIRST acquisition attempt hit
    /// contention.
    #[test]
    fn with_retry_succeeds_once_a_concurrent_write_lock_releases() {
        let dir = crate::conn::test_temp_dir("retry-acquire");
        let path = dir.join("retry-acquire.db");

        let mut conn = crate::conn::open_recovery_store(crate::conn::RecoveryTarget::File(&path))
            .expect("open contending connection");
        let mut blocker =
            crate::conn::open_recovery_store(crate::conn::RecoveryTarget::File(&path))
                .expect("open blocker connection");

        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let held = blocker
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .expect("blocker acquires the write lock first");
            ready_tx.send(()).expect("signal ready");
            release_rx.recv().expect("wait for release signal");
            held.commit().expect("release the write lock");
        });
        ready_rx
            .recv()
            .expect("wait for the blocker to hold the lock");

        let now = crate::session::format_rfc3339_nanos(std::time::SystemTime::now());

        // Release the blocker's lock from a second thread, timed by a
        // rendezvous rather than a wall-clock sleep: it waits for OUR
        // signal, which we send only once we're about to attempt the
        // acquisition below — no pacing sleep on either side.
        release_tx.send(()).expect("signal release");

        let id = with_retry(&mut conn, |tx| {
            tx.execute(
                "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/a.md', ?1, ?1)",
                [&now],
            )?;
            Ok(tx.last_insert_rowid())
        })
        .expect("with_retry must succeed once the concurrent lock releases");
        assert_eq!(id, 1);

        holder.join().expect("holder thread must not panic");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_retry_commits_a_successful_op() {
        let mut conn = crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(
            &crate::conn::memory_uri(),
        ))
        .expect("open");
        let now = crate::session::format_rfc3339_nanos(std::time::SystemTime::now());
        let id = with_retry(&mut conn, |tx| {
            tx.execute(
                "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('/a.md', ?1, ?1)",
                [&now],
            )?;
            Ok(tx.last_insert_rowid())
        })
        .expect("with_retry");
        assert_eq!(id, 1);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 1);
    }

    #[test]
    fn with_retry_rolls_back_and_surfaces_a_non_retryable_error() {
        let mut conn = crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(
            &crate::conn::memory_uri(),
        ))
        .expect("open");
        let result: Result<(), Error> = with_retry(&mut conn, |tx| {
            // kind is CHECK-constrained to a fixed set — this violates it.
            tx.execute(
                "INSERT INTO documents(path, kind, created_at, last_seen_at) VALUES ('/a.md', 'bogus', 'x', 'x')",
                [],
            )?;
            Ok(())
        });
        assert!(result.is_err());

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 0, "the failed insert must not have committed");
    }
}
