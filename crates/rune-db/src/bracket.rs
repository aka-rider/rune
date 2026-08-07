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

/// `doc_id`'s newest recorded observation's own blob hash, of ANY confirmed
/// status — deliberately unlike [`newest_confirmed_size`], since a shrink
/// hypothesis this function helps validate is itself recorded unconfirmed
/// and must still be visible as "the thing sighted last time".
fn newest_observation_hash(tx: &Transaction<'_>, doc_id: i64) -> Result<Option<String>, Error> {
    tx.query_row(
        "SELECT blob_hash FROM observations WHERE doc_id=?1 ORDER BY id DESC LIMIT 1",
        params![doc_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(Error::from)
}

/// Folds the suspicious-shrink gate into a bracket's own `confirmed` verdict:
/// a bracket-stable read (`bracket_confirmed`) that is empty or radically
/// shrunk relative to `doc_id`'s newest CONFIRMED observation does not
/// automatically inherit that confirmation — the destructive-async-reset
/// pattern a stable stat bracket alone cannot see, since the file's
/// identity can legitimately change across an ordinary external rewrite.
/// A shrink is a HYPOTHESIS the first time it's sighted (recorded
/// unconfirmed, so nothing downstream trusts it yet) and VALIDATED the
/// moment an independent bracketed read sights byte-identical content again
/// (`new_hash` equal to the newest recorded observation's hash, whatever its
/// own confirmed status) — a legitimate external rewrite that shrank the
/// file settles on the same bytes across two separate reads, while a
/// transient mid-rewrite artifact does not repeat identically. An unstable
/// bracket is never upgraded by this check; it stays unconfirmed regardless
/// of length.
pub fn confirm_against_history(
    tx: &Transaction<'_>,
    doc_id: i64,
    bracket_confirmed: bool,
    new_len: usize,
    new_hash: &str,
) -> Result<bool, Error> {
    if !bracket_confirmed {
        return Ok(false);
    }
    let baseline = newest_confirmed_size(tx, doc_id)?;
    if !baseline.is_some_and(|before| rune_core::is_suspicious_shrink(before as usize, new_len)) {
        return Ok(true);
    }
    Ok(newest_observation_hash(tx, doc_id)?.is_some_and(|prior| prior == new_hash))
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
    let hash = observation::hash_bytes(&bracket.data);
    let at = format_rfc3339_nanos(now);
    retry::with_retry(conn, |tx| {
        let confirmed =
            confirm_against_history(tx, doc_id, bracket.confirmed, bracket.data.len(), &hash)?;
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
    use rune_vfs::{DirEntry, Mem, OpKind, Stat};

    /// Wraps a `Mem`, delegating every `Vfs` method to it verbatim EXCEPT
    /// `stat`, which always fails — a stat that never recovers, unlike
    /// `Mem::fail_next`'s one-shot injection, so a bracket's retry loop
    /// genuinely exhausts every attempt against a persistently unavailable
    /// stat.
    struct AlwaysFailStatVfs {
        inner: Mem,
    }

    impl Vfs for AlwaysFailStatVfs {
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.inner.read(path)
        }
        fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<std::path::PathBuf> {
            self.inner.write_durable(path, bytes)
        }
        fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
            self.inner.exchange(a, b)
        }
        fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()> {
            self.inner.rename_excl(old, new)
        }
        fn remove(&self, path: &Path) -> io::Result<()> {
            self.inner.remove(path)
        }
        fn trash(&self, path: &Path) -> io::Result<()> {
            self.inner.trash(path)
        }
        fn stat(&self, _path: &Path) -> io::Result<Stat> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "stat always fails",
            ))
        }
        fn resolve(&self, path: &Path) -> io::Result<std::path::PathBuf> {
            self.inner.resolve(path)
        }
        fn mkdir_all(&self, path: &Path) -> io::Result<()> {
            self.inner.mkdir_all(path)
        }
        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            self.inner.read_dir(path)
        }
    }

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

    /// A TRANSIENT stat failure never confirms the attempt it hits, but the
    /// bracket's retry recovers: `Mem::fail_next(OpKind::Stat, ..)` fires
    /// once, so only the bracket's FIRST stat fails; the retry loop still
    /// needs the second attempt's own pair of stats to succeed and agree
    /// for `confirmed` to come back `true` — proving a failed stat is
    /// excluded from, not silently absorbed into, the comparison
    /// (`StatFacts::default()` on error compares equal to itself, which is
    /// exactly the bug this bracket must not reproduce).
    #[test]
    fn a_transient_stat_failure_is_excluded_from_the_comparison_and_the_retry_recovers() {
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

    /// A stat failure that PERSISTS across every bounded attempt — unlike
    /// `Mem::fail_next`'s one-shot injection — must exhaust the retry loop
    /// and never confirm: `StatFacts::default()` on error compares equal to
    /// itself, which is exactly the bug this bracket must not reproduce.
    #[test]
    fn stat_failures_that_persist_across_every_attempt_never_confirm() {
        let vfs = AlwaysFailStatVfs { inner: Mem::new() };
        let path = Path::new("/doc.md");
        publish(&vfs.inner, path, b"hello");

        let bracket = bracketed_read(&vfs, path).expect("bracketed_read");
        assert!(
            !bracket.confirmed,
            "a stat that never recovers must exhaust the retry loop unconfirmed"
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

    fn seed_confirmed(tx: &Transaction<'_>, doc_id: i64, session_id: i64, content: &str) {
        let hash = seed_blob(tx, content);
        observation::record_observation(
            tx,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &hash,
                seq: None,
                origin: "probe",
                confirmed: Some(true),
            },
            &StatFacts {
                size: content.len() as i64,
                mtime: "t".to_string(),
                ..Default::default()
            },
            "t",
        )
        .expect("seed confirmed observation");
    }

    /// A shrink's FIRST sighting is a hypothesis, not a fact: even a
    /// perfectly bracket-stable empty read against non-empty confirmed
    /// history is downgraded to unconfirmed the first time it's seen.
    #[test]
    fn confirm_against_history_downgrades_the_first_sighting_of_a_shrink() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        seed_confirmed(&tx, doc_id, session_id, "a whole paragraph of real content");

        let empty_hash = observation::hash_bytes(b"");
        let confirmed =
            confirm_against_history(&tx, doc_id, true, 0, &empty_hash).expect("confirm");
        assert!(
            !confirmed,
            "the first sighting of a shrink against confirmed history is only a hypothesis"
        );
        tx.commit().expect("commit");
    }

    /// A shrink's SECOND identical sighting validates the hypothesis: once
    /// an independent bracketed read has already recorded the shrunk
    /// content once (however unconfirmed), a fresh read that sees the exact
    /// same bytes again is a legitimate external rewrite settling, not a
    /// transient artifact, and confirms.
    #[test]
    fn confirm_against_history_validates_a_second_identical_shrink_sighting() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        seed_confirmed(&tx, doc_id, session_id, "a whole paragraph of real content");

        let shrunk_hash = seed_blob(&tx, "short");
        observation::record_observation(
            &tx,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &shrunk_hash,
                seq: None,
                origin: "probe",
                confirmed: Some(false),
            },
            &StatFacts {
                size: 5,
                mtime: "t2".to_string(),
                ..Default::default()
            },
            "t2",
        )
        .expect("seed first shrink hypothesis");

        let confirmed =
            confirm_against_history(&tx, doc_id, true, 5, &shrunk_hash).expect("confirm");
        assert!(
            confirmed,
            "a second bracketed read of byte-identical shrunk content validates the hypothesis"
        );
        tx.commit().expect("commit");
    }

    /// A transient mid-rewrite empty read never confirms even when a LATER
    /// read restores the original content — the restoration isn't a
    /// shrink at all (so it confirms on its own, unrelated grounds), but
    /// the earlier empty sighting itself is never revisited or upgraded.
    #[test]
    fn confirm_against_history_never_confirms_a_transient_empty_read_via_restoration() {
        let mut conn = open();
        let session_id =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);
        let original = "a whole paragraph of real content";
        seed_confirmed(&tx, doc_id, session_id, original);

        let empty_hash = seed_blob(&tx, "");
        let empty_confirmed =
            confirm_against_history(&tx, doc_id, true, 0, &empty_hash).expect("confirm empty");
        assert!(
            !empty_confirmed,
            "the transient empty read stays unconfirmed"
        );
        observation::record_observation(
            &tx,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &empty_hash,
                seq: None,
                origin: "probe",
                confirmed: Some(false),
            },
            &StatFacts {
                size: 0,
                mtime: "t2".to_string(),
                ..Default::default()
            },
            "t2",
        )
        .expect("record the empty hypothesis");

        let restored_hash = observation::hash_bytes(original.as_bytes());
        let restored_confirmed =
            confirm_against_history(&tx, doc_id, true, original.len(), &restored_hash)
                .expect("confirm restored");
        assert!(
            restored_confirmed,
            "restoring the original content is not a shrink and confirms on its own"
        );
        tx.commit().expect("commit");
    }

    #[test]
    fn confirm_against_history_never_upgrades_an_unstable_bracket() {
        let mut conn = open();
        let tx = conn.transaction().expect("tx");
        let doc_id = seed_doc(&tx);

        let confirmed =
            confirm_against_history(&tx, doc_id, false, 100, "irrelevant").expect("confirm");
        assert!(!confirmed);
        tx.commit().expect("commit");
    }
}
