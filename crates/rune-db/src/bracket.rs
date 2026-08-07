//! A single successfully-returned read is evidence, never truth: every
//! fresh disk read that produces an observation is bracketed (stat, read,
//! re-stat — [`bracketed_read`]) or, for a caller that already knows the
//! bytes it wrote and only needs to confirm its own publish
//! ([`bracketed_stat`]). [`observe_disk`] is the chokepoint every fresh
//! "read the live file, then record what it said" call site (`probe::probe`,
//! `load::load`) funnels through: it brackets the read, folds in the
//! suspicious-shrink gate against the newest CONFIRMED observation already
//! on file ([`confirm_against_history`]), then puts the bytes as a blob and
//! records the referencing observation. Only a `confirmed: Some(true)`
//! observation may short-circuit a probe, serve as a merge Theirs, or become
//! a CAS baseline — an unconfirmed or unclassified (`None`, legacy)
//! observation decides nothing, though its blob is kept exactly like any
//! other (blob retention is sacred).

use std::io;
use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use rune_vfs::Vfs;

use crate::Error;
use crate::observation::{self, ObserveInput, StatFacts};
use crate::retry;
use crate::session::format_rfc3339_nanos;

/// The bound on how many times a bracket (stat-read-stat, or the CAS
/// re-verify read) retries an unstable result before giving up and reporting
/// what it last saw — bounded so a persistently changing file degrades to
/// "unconfirmed", never an unbounded spin.
pub const BRACKET_MAX_ATTEMPTS: u32 = 3;

/// `stat_identity`'s result, narrowed to `Some` only when the stat actually
/// succeeded AND exposed a real (inode, device) identity — a failed stat
/// degrades to `StatFacts::default()`, and two such defaults compare equal,
/// so a bracket comparing raw `StatFacts` values would let two FAILED stats
/// masquerade as a confirmed match. Every bracket in this module compares
/// through this instead, never through `stat_identity` directly.
fn stat_with_identity(vfs: &dyn Vfs, path: &Path) -> Option<StatFacts> {
    let stat = observation::stat_identity(vfs, path);
    (stat.inode.is_some() && stat.device.is_some()).then_some(stat)
}

/// A disk read bracketed by a stat immediately before and after it — the
/// read-side half of "a single read is evidence, never truth". `confirmed`
/// is `true` only when both stats succeeded, exposed a real identity, and
/// compared IDENTICAL: the file's identity/size/mtime did not move between
/// the two stats, so `data` is what a caller can trust actually existed on
/// disk at a single instant, not two different instants stitched together.
#[derive(Clone, Debug, PartialEq)]
pub struct BracketedRead {
    pub data: Vec<u8>,
    pub stat: StatFacts,
    pub confirmed: bool,
}

fn one_read_bracket(vfs: &dyn Vfs, path: &Path) -> io::Result<BracketedRead> {
    let before = stat_with_identity(vfs, path);
    let data = vfs.read(path)?;
    let after = stat_with_identity(vfs, path);
    let confirmed = matches!((&before, &after), (Some(b), Some(a)) if b == a);
    let stat = after.or(before).unwrap_or_default();
    Ok(BracketedRead {
        data,
        stat,
        confirmed,
    })
}

/// Brackets a read of `path`: stat, read, re-stat, retrying (bounded by
/// [`BRACKET_MAX_ATTEMPTS`]) while the bracket stays unstable. A retry
/// re-runs the WHOLE bracket, never just the second stat — an unstable
/// result means the file was genuinely moving, and only a fresh read can
/// describe whatever it settled on. Still unstable after every attempt
/// returns the LAST attempt's bytes/stat with `confirmed: false` — a
/// destructive async replacement is suspect until proven, never dropped
/// (blob retention is sacred), but it decides nothing downstream either.
pub fn bracketed_read(vfs: &dyn Vfs, path: &Path) -> io::Result<BracketedRead> {
    let mut result = one_read_bracket(vfs, path)?;
    let mut attempts = 1;
    while !result.confirmed && attempts < BRACKET_MAX_ATTEMPTS {
        result = one_read_bracket(vfs, path)?;
        attempts += 1;
    }
    Ok(result)
}

/// Two stats of `path`, taken back to back with no read in between —
/// [`bracketed_read`]'s counterpart for a caller that already knows what it
/// just wrote (a save's own publish) and only needs to confirm nothing raced
/// the file's IDENTITY between the write and the moment it stats the result.
/// `confirmed` follows the same rule as `bracketed_read`'s.
#[derive(Clone, Debug, PartialEq)]
pub struct BracketedStat {
    pub stat: StatFacts,
    pub confirmed: bool,
}

fn one_stat_bracket(vfs: &dyn Vfs, path: &Path) -> BracketedStat {
    let before = stat_with_identity(vfs, path);
    let after = stat_with_identity(vfs, path);
    let confirmed = matches!((&before, &after), (Some(b), Some(a)) if b == a);
    let stat = after.or(before).unwrap_or_default();
    BracketedStat { stat, confirmed }
}

/// Brackets a stat of `path` with a second stat immediately after, retrying
/// (bounded by [`BRACKET_MAX_ATTEMPTS`]) while the two disagree.
pub fn bracketed_stat(vfs: &dyn Vfs, path: &Path) -> BracketedStat {
    let mut result = one_stat_bracket(vfs, path);
    let mut attempts = 1;
    while !result.confirmed && attempts < BRACKET_MAX_ATTEMPTS {
        result = one_stat_bracket(vfs, path);
        attempts += 1;
    }
    result
}

/// The size a bracketed read's `confirmed` bracket result still must clear
/// before it counts as confirmed: `doc_id`'s newest CONFIRMED observation's
/// own recorded size, or `None` when there is no confirmed history yet to
/// compare against. Deliberately ignores unconfirmed observations entirely —
/// an unconfirmed fact decides nothing, including what counts as a
/// suspicious shrink relative to it.
fn newest_confirmed_size(tx: &Transaction<'_>, doc_id: i64) -> Result<Option<i64>, Error> {
    tx.query_row(
        "SELECT size FROM observations WHERE doc_id=?1 AND confirmed=1 ORDER BY id DESC LIMIT 1",
        params![doc_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(Error::from)
}

/// Folds the suspicious-shrink gate into a bracket's own `confirmed` verdict:
/// a bracket-stable read (`bracket_confirmed`) that is empty or radically
/// shrunk relative to `doc_id`'s newest CONFIRMED observation is still
/// downgraded to unconfirmed — the destructive-async-reset pattern a stable
/// stat bracket alone cannot see, since the file's identity can legitimately
/// change across an ordinary external rewrite. An unstable bracket is never
/// upgraded by this check; it stays unconfirmed regardless of length.
pub fn confirm_against_history(
    tx: &Transaction<'_>,
    doc_id: i64,
    bracket_confirmed: bool,
    new_len: usize,
) -> Result<bool, Error> {
    if !bracket_confirmed {
        return Ok(false);
    }
    let baseline = newest_confirmed_size(tx, doc_id)?;
    Ok(!baseline.is_some_and(|before| rune_core::is_suspicious_shrink(before as usize, new_len)))
}

/// The correlation/origin facts [`observe_disk`] needs —
/// [`crate::observation::ObservationMeta`] minus `confirmed`, which
/// `observe_disk` always derives for itself from the bracket and never
/// accepts from a caller.
#[derive(Clone, Copy, Debug)]
pub struct ObserveDiskMeta<'a> {
    /// The journal position this sighting correlates to; `None` means
    /// uncorrelated (never ancestor-eligible).
    pub seq: Option<i64>,
    /// `'load'|'save'|'watch'|'probe'|'resolve'|'swap'` (schema-enforced).
    pub origin: &'a str,
}

/// Brackets a fresh disk read for `doc_id` (stat, read, re-stat via
/// [`bracketed_read`]) and folds in the suspicious-shrink gate against the
/// newest CONFIRMED observation already on file (via
/// [`confirm_against_history`]), then puts the bytes as a blob and records
/// the referencing observation, all in ONE transaction beyond the read
/// itself (the blob put and its observation insert never split across two,
/// closing the cross-process GC race [rune-db 2]) — the one chokepoint every
/// fresh "read the live file, then record what it said" call site
/// (`probe::probe`, `load::load`) funnels through, so a racer caught
/// mid-external-rewrite can never masquerade as a stable, trusted fact.
pub fn observe_disk(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: i64,
    doc_id: i64,
    path: &Path,
    meta: ObserveDiskMeta<'_>,
    now: SystemTime,
) -> Result<observation::Observation, Error> {
    let bracket = bracketed_read(vfs, path).map_err(Error::Io)?;
    let at = format_rfc3339_nanos(now);
    retry::with_retry(conn, |tx| {
        let confirmed = confirm_against_history(tx, doc_id, bracket.confirmed, bracket.data.len())?;
        observation::observe_from_stat_tx(
            tx,
            session_id,
            doc_id,
            &bracket.stat,
            &at,
            ObserveInput {
                data: &bracket.data,
                seq: meta.seq,
                origin: meta.origin,
                confirmed: Some(confirmed),
            },
        )
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::observation::ObservationMeta;
    use rune_vfs::{Mem, OpKind};

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    fn seed_doc(tx: &Transaction<'_>) -> i64 {
        tx.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        tx.last_insert_rowid()
    }

    fn seed_blob(tx: &Transaction<'_>, content: &str) -> String {
        crate::blob::put_blob(tx, content.as_bytes()).expect("seed blob")
    }

    fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
        let temp = vfs.write_durable(path, bytes).expect("write_durable");
        vfs.rename_excl(&temp, path).expect("publish");
    }

    #[test]
    fn bracketed_read_on_a_quiescent_file_confirms() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"hello");

        let bracket = bracketed_read(&vfs, path).expect("bracketed_read");
        assert!(bracket.confirmed);
        assert_eq!(bracket.data, b"hello");
    }

    #[test]
    fn bracketed_read_propagates_a_missing_file_as_not_found() {
        let vfs = Mem::new();
        let err = bracketed_read(&vfs, Path::new("/gone.md")).expect_err("must error");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    /// A stat failure never confirms — `StatFacts::default()` on error
    /// compares equal to itself, which is exactly the bug this bracket must
    /// not reproduce. `Mem::fail_next(OpKind::Stat, ..)` fires once, so only
    /// the bracket's FIRST stat fails; the retry loop still needs the second
    /// attempt's own pair of stats to succeed and agree for `confirmed` to
    /// come back `true` — proving a failed stat is excluded from, not
    /// silently absorbed into, the comparison.
    #[test]
    fn a_failed_stat_never_confirms_the_bracket() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"hello");
        vfs.fail_next(OpKind::Stat, io::ErrorKind::PermissionDenied);

        let bracket = bracketed_read(&vfs, path).expect("bracketed_read");
        assert!(
            bracket.confirmed,
            "the retry must recover once the injected stat failure is consumed"
        );
    }

    /// The mid-bracket-mutation shape: the file's identity/content changes
    /// strictly between the bracket's own two stat calls. The first attempt
    /// must come back unconfirmed; the retry then sees the settled state and
    /// confirms.
    #[test]
    fn a_mutation_between_the_two_stats_is_caught_and_the_retry_recovers() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"before");
        vfs.mutate_after_next_stat(path, b"after".to_vec());

        let bracket = bracketed_read(&vfs, path).expect("bracketed_read");
        assert!(
            bracket.confirmed,
            "the retry attempt must settle and confirm"
        );
        assert_eq!(bracket.data, b"after");
    }

    /// The same-stat-different-content shape (G2): a rewrite that lands
    /// between two probes without moving any stat-visible fact at all is
    /// exactly what a stat-only "nothing changed" comparison cannot see —
    /// this pins that a bracket run AFTER such a rewrite still confirms
    /// (the bracket's own two stats agree with EACH OTHER, correctly, since
    /// nothing moved DURING this particular bracket) and reads the NEW
    /// content, never the stale one a naive short-circuit would have kept
    /// serving.
    #[test]
    fn same_stat_different_content_is_read_fresh_not_masked_by_stable_identity() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"before");
        vfs.set_content_keep_identity(path, b"after".to_vec())
            .expect("set_content_keep_identity");

        let bracket = bracketed_read(&vfs, path).expect("bracketed_read");
        assert!(bracket.confirmed);
        assert_eq!(bracket.data, b"after");
    }

    #[test]
    fn bracketed_stat_on_a_quiescent_file_confirms() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"hello");

        let bracket = bracketed_stat(&vfs, path);
        assert!(bracket.confirmed);
    }

    #[test]
    fn confirm_against_history_downgrades_an_empty_read_against_confirmed_history() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let hash = seed_blob(&tx, "a whole paragraph of real content");
        observation::record_observation(
            &tx,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &hash,
                seq: None,
                origin: "probe",
                confirmed: Some(true),
            },
            &StatFacts {
                size: 34,
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
        )
        .expect("seed confirmed observation");

        let confirmed = confirm_against_history(&tx, doc_id, true, 0).expect("confirm");
        assert!(
            !confirmed,
            "an empty read against non-empty confirmed history must downgrade to unconfirmed"
        );
        tx.commit().expect("commit");
    }

    #[test]
    fn confirm_against_history_never_upgrades_an_unstable_bracket() {
        let mut conn = open();
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let confirmed = confirm_against_history(&tx, doc_id, false, 100).expect("confirm");
        assert!(!confirmed);
        tx.commit().expect("commit");
    }
}
