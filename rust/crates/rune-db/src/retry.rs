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
pub enum Classification {
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
pub fn classify(err: &rusqlite::Error) -> Classification {
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
/// binding rule, Go invariant I1).
pub fn with_retry<T>(
    conn: &mut Connection,
    mut op: impl FnMut(&Transaction) -> Result<T, Error>,
) -> Result<T, Error> {
    let mut restart_attempts = 0u32;
    let mut backoff_attempts = 0u32;

    loop {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        match op(&tx) {
            Ok(value) => {
                tx.commit()?;
                return Ok(value);
            }
            Err(err) => {
                let _ = tx.rollback();
                let sqlite_err = match &err {
                    Error::Sqlite(e) => Some(e),
                    _ => None,
                };
                let Some(sqlite_err) = sqlite_err else {
                    return Err(err);
                };
                match classify(sqlite_err) {
                    Classification::RestartTransaction => {
                        restart_attempts += 1;
                        if restart_attempts > MAX_RESTART_ATTEMPTS {
                            return Err(err);
                        }
                        continue;
                    }
                    Classification::Backoff => {
                        backoff_attempts += 1;
                        if backoff_attempts > MAX_BACKOFF_ATTEMPTS {
                            return Err(err);
                        }
                        std::thread::sleep(jittered_backoff(backoff_attempts));
                        continue;
                    }
                    Classification::Surface => return Err(err),
                }
            }
        }
    }
}

/// A short, monotonically-growing backoff with a little jitter so multiple
/// contending connections don't retry in lockstep. Not seeded from an
/// injectable clock (unlike the coalescing clock elsewhere in this crate):
/// this delay paces real cross-process lock contention, not something a
/// test should ever need to control deterministically — the same way
/// rusqlite's own `busy_timeout` isn't either.
fn jittered_backoff(attempt: u32) -> Duration {
    let base_ms = 5u64 * u64::from(attempt);
    let jitter_ms = u64::from(std::process::id()) % 8;
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

    #[test]
    fn with_retry_commits_a_successful_op() {
        let mut conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
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
        let mut conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
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
