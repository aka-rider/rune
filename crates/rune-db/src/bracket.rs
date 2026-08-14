//! A single successfully-returned read is evidence, never truth: every
//! fresh disk read that produces an observation goes through
//! `rune_vfs::get`'s stat/read/stat bracket ([`bracketed_read`] adapts its
//! result into this crate's [`StatFacts`] vocabulary). [`observe_disk`] is
//! the chokepoint every fresh "read the live file, then record what it
//! said" call site (`probe::probe`, `load::load`) funnels through: it
//! brackets the read, folds in the suspicious-shrink gate against the
//! newest CONFIRMED observation already on file
//! ([`confirm_against_history`]), then puts the bytes as a blob and records
//! the referencing observation. Only a confirmed observation
//! may short-circuit a probe, serve as a merge Theirs, or become a CAS
//! baseline — an unconfirmed or unclassified observation
//! decides nothing, though its blob is kept exactly like any other (blob
//! retention is sacred).

use std::io;
use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use rune_vfs::{Stat, Vfs};

use crate::Error;
use crate::confirmation::Confirmation;
use crate::ids::{DocId, SessionId};
use crate::obs_origin::ObsOrigin;
use crate::observation::{self, ObserveInput, StatFacts};
use crate::retry;
use crate::session::format_rfc3339_nanos;

pub fn stat_facts_from(stat: Option<Stat>) -> StatFacts {
    stat.map_or_else(StatFacts::default, |st| StatFacts {
        size: Some(st.size as i64),
        mtime: Some(format_rfc3339_nanos(st.mtime)),
        inode: st.identity.inode.map(|v| v as i64),
        device: st.identity.device.map(|v| v as i64),
        nlink: st.nlink.map(|v| v as i64),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct BracketedRead {
    pub data: Vec<u8>,
    pub stat: StatFacts,
    pub confirmed: bool,
}

pub fn bracketed_read(vfs: &dyn Vfs, path: &Path) -> io::Result<BracketedRead> {
    match rune_vfs::get(vfs, path, None) {
        Ok(sighting) => Ok(BracketedRead {
            data: sighting.bytes,
            stat: stat_facts_from(sighting.sighted.stat()),
            confirmed: sighting.sighted.is_confirmed(),
        }),
        Err(refusal) => Err(refusal.into()),
    }
}

/// The size a bracketed read's `confirmed` bracket result still must clear
/// before it counts as confirmed: `doc_id`'s newest CONFIRMED observation's
/// own recorded size, or `None` when there is no confirmed history yet to
/// compare against. Deliberately ignores unconfirmed observations entirely —
/// an unconfirmed fact decides nothing, including what counts as a
/// suspicious shrink relative to it.
fn newest_confirmed_size(tx: &Transaction<'_>, doc_id: DocId) -> Result<Option<i64>, Error> {
    tx.query_row(
        "SELECT size FROM observations WHERE doc_id=?1 AND confirmed=1 ORDER BY id DESC LIMIT 1",
        params![doc_id],
        |r| r.get::<_, Option<i64>>(0),
    )
    .optional()
    .map(Option::flatten)
    .map_err(Error::from)
}

/// `doc_id`'s newest recorded observation's own blob hash, of ANY confirmed
/// status — deliberately unlike [`newest_confirmed_size`], since a shrink
/// hypothesis this function helps validate is itself recorded unconfirmed
/// and must still be visible as "the thing sighted last time".
fn newest_observation_hash(tx: &Transaction<'_>, doc_id: DocId) -> Result<Option<String>, Error> {
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
    doc_id: DocId,
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
pub struct ObserveDiskMeta {
    /// The journal position this sighting correlates to; `None` means
    /// uncorrelated (never ancestor-eligible).
    pub seq: Option<i64>,
    pub origin: ObsOrigin,
}

/// Brackets a fresh disk read for `doc_id` (via [`bracketed_read`]) and
/// folds in the suspicious-shrink gate against the newest CONFIRMED
/// observation already on file (via [`confirm_against_history`]), then puts
/// the bytes as a blob and records the referencing observation, all in ONE
/// transaction beyond the read itself (the blob put and its observation
/// insert never split across two, closing the cross-process GC race
/// [rune-db 2]) — the one chokepoint every fresh "read the live file, then
/// record what it said" call site (`probe::probe`, `load::load`) funnels
/// through, so a racer caught mid-external-rewrite can never masquerade as
/// a stable, trusted fact.
pub fn observe_disk(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: SessionId,
    doc_id: DocId,
    path: &Path,
    meta: ObserveDiskMeta,
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
                confirmed: Confirmation::from_bracket(confirmed),
            },
        )
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::observation::ObservationMeta;
    use rune_vfs::Mem;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        crate::schema::apply(&conn).expect("schema");
        conn
    }

    fn seed_doc(tx: &Transaction<'_>) -> DocId {
        tx.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        DocId(tx.last_insert_rowid())
    }

    fn seed_blob(tx: &Transaction<'_>, content: &str) -> String {
        crate::blob::put_blob(tx, content.as_bytes()).expect("seed blob")
    }

    fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
        let temp = vfs.write_durable(path, bytes).expect("write_durable");
        vfs.rename_excl(&temp, path).expect("publish");
    }

    #[test]
    fn bracketed_read_on_a_quiescent_file_confirms_with_real_stat_facts() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"hello");

        let bracket = bracketed_read(&vfs, path).expect("bracketed_read");
        assert!(bracket.confirmed);
        assert_eq!(bracket.data, b"hello");
        assert_eq!(bracket.stat.size, Some(5));
        assert!(bracket.stat.mtime.is_some());
    }

    #[test]
    fn bracketed_read_propagates_a_missing_file_as_not_found() {
        let vfs = Mem::new();
        let err = bracketed_read(&vfs, Path::new("/gone.md")).expect_err("must error");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn bracketed_read_refuses_a_non_file() {
        let vfs = Mem::new();
        publish(&vfs, Path::new("/fifo"), b"");
        vfs.set_kind(Path::new("/fifo"), rune_vfs::FileKind::Other)
            .expect("set_kind");

        let err = bracketed_read(&vfs, Path::new("/fifo")).expect_err("must refuse");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn an_unstable_bracket_reports_unconfirmed() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"before");
        vfs.set_churning(path, true);

        let bracket = bracketed_read(&vfs, path).expect("bracketed_read");
        assert!(!bracket.confirmed);
    }

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
    fn stat_facts_from_none_is_all_null_never_a_synthetic_zero_size() {
        let facts = stat_facts_from(None);
        assert_eq!(facts.size, None);
        assert_eq!(facts.mtime, None);
        assert_eq!(facts.inode, None);
        assert_eq!(facts.device, None);
        assert_eq!(facts.nlink, None);
    }

    fn seed_confirmed(tx: &Transaction<'_>, doc_id: DocId, session_id: SessionId, content: &str) {
        let hash = seed_blob(tx, content);
        observation::record_observation(
            tx,
            doc_id,
            session_id,
            ObservationMeta {
                blob_hash: &hash,
                seq: None,
                origin: ObsOrigin::Probe,
                confirmed: Confirmation::Confirmed,
            },
            &StatFacts {
                size: Some(content.len() as i64),
                mtime: Some("t".to_string()),
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
                origin: ObsOrigin::Probe,
                confirmed: Confirmation::Unconfirmed,
            },
            &StatFacts {
                size: Some(5),
                mtime: Some("t2".to_string()),
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
                origin: ObsOrigin::Probe,
                confirmed: Confirmation::Unconfirmed,
            },
            &StatFacts {
                size: Some(0),
                mtime: Some("t2".to_string()),
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
