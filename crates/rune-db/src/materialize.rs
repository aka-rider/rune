//! `Materialize` — the CAS write protocol that turns a buffer into the
//! user's destination file. WP7 inverted this module's shape: it used to
//! run the ENTIRE protocol — including the `vfs.write_durable`/`exchange`
//! disk publish itself — as one op on the writer thread's single FIFO, so a
//! dead writer thread made saving impossible even though the publish itself
//! needs nothing from the database ([rune-db 1]). The disk publish now runs
//! on the CALLER's own thread (`rune-tui`'s save `Cmd`, via its OWN `Vfs`
//! handle), and this module is bookkeeping-only around it:
//!
//! - [`prepare_materialize`] — pure DB read, no `vfs` call at all: hands the
//!   caller the CAS baseline (`expect`'s hash) and the bound path to check
//!   its own target against, before it does any disk I/O.
//! - The caller performs the actual `resolve`/read/hash-compare/
//!   `write_durable`/`exchange`(or `rename_excl`)/read-displaced dance
//!   itself, using [`rune_vfs::published_not_durable`] to tell "the swap
//!   already took effect" apart from "it never happened" the same way
//!   `Vfs::save_atomic` does.
//! - [`record_materialize_outcome`] — records what the caller's vfs work
//!   concluded (a conflict, a plain commit, or a swap-race commit) as the
//!   same CAS bookkeeping `commit_save`/`record_fresh` always did, now fed
//!   caller-supplied bytes/stat facts instead of calling `vfs` itself.
//!
//! A dead writer thread can still fail [`prepare_materialize`] or
//! [`record_materialize_outcome`]'s enqueue (`Error::WriterGone`) — but by
//! then the disk publish is either not yet attempted (prepare failed: the
//! caller falls back to an uncoordinated direct write, same as a document
//! with no store binding at all) or already physically complete (record
//! failed: the user's bytes are safely on disk; only this session's CAS
//! bookkeeping is lost, which degrades the store, not the save). Every
//! `vfs` call this module used to make is gone; the sibling relation
//! `lib.rs` documents is now structural, not just a convention two modules
//! happen to follow.

use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, params};

use crate::Error;
use crate::confirmation::Confirmation;
use crate::ids::{BlobHash, ObsId};
use crate::obs_origin::ObsOrigin;
use crate::observation::{self, Observation, ObservationMeta, StatFacts};
use crate::rebind::{Rebind, rebind_document_tx};
use crate::retry;

pub(crate) use crate::materialize_types::DocSession;
pub use crate::materialize_types::{
    MatResult, MaterializeOutcome, MaterializePrep, MaterializeTarget,
};

/// Step 1 (now caller-facing): fetches the decision data for an `Existing`
/// materialize attempt — the bound path to check the caller's own target
/// against, the baseline hash to CAS-compare the live target against, and
/// this session's own [`crate::sync::sync`] classification, all read in ONE
/// transaction so the caller never sees a baseline and a verdict taken from
/// two different states of the database.
/// Pure DB read, no `vfs` call ([rune-db 1]'s fix: this can run
/// even while every disk in the workspace is unreachable, and the caller's
/// OWN subsequent disk work never depends on the writer thread being alive
/// a moment longer than it takes to answer this one query).
///
/// `pending_rebaseline_hash`, when given, stands in for `expect`'s own
/// stored hash: the caller's own last publish physically committed but the
/// observation that would have advanced `expect` past it was lost to a
/// failing writer, so `expect`'s row still names the PRE-publish content.
/// Comparing against it here, inside the same transaction the rest of this
/// decision is read from, is what lets a save that starts before the
/// re-baseline `Load` lands recognize the disk as its own echo instead of
/// manufacturing a conflict against bytes this session just wrote.
pub(crate) fn prepare_materialize(
    conn: &mut Connection,
    ds: DocSession,
    target: MaterializeTarget,
    pending_rebaseline_hash: Option<BlobHash>,
) -> Result<MaterializePrep, Error> {
    let expect = match target {
        MaterializeTarget::BindNew => return Ok(MaterializePrep::Create),
        MaterializeTarget::Existing { expect } => expect,
    };

    retry::with_retry(conn, |tx| {
        let db_path: String = tx.query_row(
            "SELECT path FROM documents WHERE id=?1",
            params![ds.doc_id],
            |r| r.get(0),
        )?;
        if db_path.is_empty() {
            return Err(Error::Invalid(format!(
                "materialize doc {}: no path bound (untitled document)",
                ds.doc_id
            )));
        }
        let expect_hash = match &pending_rebaseline_hash {
            Some(hash) => hash.clone(),
            None => observation::get_observation(tx, expect)?.blob_hash,
        };
        let sync = crate::sync::sync(tx, ds.session_id, ds.doc_id)?;
        Ok(MaterializePrep::Overwrite {
            bound_path: db_path,
            expect_hash,
            sync: sync.kind,
        })
    })
}

/// Steps 4-5 (now caller-facing): records what the caller's own `vfs` work
/// concluded — a CAS conflict, a plain commit, or a swap-race commit — as
/// the same blob+observation(+rebind) bookkeeping `commit_save`/
/// `record_fresh` always did, fed caller-supplied bytes/stat facts instead
/// of calling `vfs`. `resolved_path`/`seq` are the caller's own
/// enqueue-time-captured facts, never re-derived here.
/// `resolved_path` is the caller's own already-`vfs.resolve`d destination —
/// converted to the checked `TEXT`-column string here (A4, [rune-db 6]: a
/// non-UTF-8 path is rejected loudly rather than mangled), the one place
/// this module still needs a `Path` at all, and it never touches disk to
/// produce it.
pub(crate) fn record_materialize_outcome(
    conn: &mut Connection,
    ds: DocSession,
    resolved_path: &Path,
    seq: i64,
    now: SystemTime,
    outcome: MaterializeOutcome,
) -> Result<MatResult, Error> {
    match outcome {
        MaterializeOutcome::Conflict {
            data,
            origin,
            stat,
            confirmed,
        } => {
            let fresh = record_fresh_from_stat(
                conn,
                ds,
                &data,
                origin,
                &stat,
                Confirmation::from_bracket(confirmed),
                now,
            )?;
            Ok(MatResult::Refused { fresh })
        }
        MaterializeOutcome::Committed {
            data,
            stat,
            confirmed,
        } => {
            let resolved_str = crate::paths::to_db_string(resolved_path)?;
            let facts = CommitFacts {
                resolved_path: &resolved_str,
                data: &data,
                seq,
                stat: &stat,
                confirmed,
                reconciled: None,
            };
            let saved = commit_save_from_stat(conn, ds, facts, now)?;
            Ok(MatResult::Committed { saved: Some(saved) })
        }
        MaterializeOutcome::Raced {
            data,
            stat,
            confirmed,
            displaced,
            displaced_stat,
        } => {
            // The displaced bytes are a one-shot read of the caller's own
            // private temp file (never contended) — recorded unclassified,
            // same as every other forensic swap capture in this
            // crate; it is never served as a decision input. Its own
            // observation IS the disk-side fact this save's commit
            // reconciles against — `facts.reconciled` below.
            let fresh = record_fresh_from_stat(
                conn,
                ds,
                &displaced,
                ObsOrigin::Swap,
                &displaced_stat,
                Confirmation::Unclassified,
                now,
            )?;
            let resolved_str = crate::paths::to_db_string(resolved_path)?;
            let facts = CommitFacts {
                resolved_path: &resolved_str,
                data: &data,
                seq,
                stat: &stat,
                confirmed,
                reconciled: Some(fresh.id),
            };
            let saved = commit_save_from_stat(conn, ds, facts, now)?;
            Ok(MatResult::CommittedRaced {
                saved,
                displaced: Box::new(fresh),
            })
        }
    }
}

/// Puts `data`'s raw bytes as a blob and records an observation of them at
/// caller-supplied `stat`, for the `Conflict{Fresh}` outcomes. `data` is
/// disk-sourced — the target's live content on a CAS refusal, or a racer's
/// displaced bytes on a swap-race; this capture happens unconditionally,
/// never gated on UTF-8 validity (see `blob.rs`'s module doc). The blob put
/// and its referencing observation insert commit as ONE
/// transaction (`observe_from_stat_tx`) — never two, closing the
/// cross-process GC race [rune-db 2]. No `vfs` call: `stat` is the
/// caller's own fact, gathered on the thread that did the actual disk work.
pub(crate) fn record_fresh_from_stat(
    conn: &mut Connection,
    ds: DocSession,
    data: &[u8],
    origin: ObsOrigin,
    stat: &StatFacts,
    confirmed: Confirmation,
    now: SystemTime,
) -> Result<Observation, Error> {
    let at = crate::session::format_rfc3339_nanos(now);
    retry::with_retry(conn, |tx| {
        observation::observe_from_stat_tx(
            tx,
            ds.session_id,
            ds.doc_id,
            stat,
            &at,
            observation::ObserveInput {
                data,
                seq: None,
                origin,
                confirmed,
            },
        )
    })
}

/// The bytes/path/seq/stat a committed write is recorded against — bundled
/// for the same argument-count reason as [`DocSession`].
#[derive(Clone, Copy, Debug)]
struct CommitFacts<'a> {
    /// The destination path, already resolved+stringified by the caller.
    resolved_path: &'a str,
    /// The bytes actually written (used to `put_blob` under the hash of
    /// what's now physically on disk).
    data: &'a [u8],
    /// The caller's save-start-captured journal position — NEVER re-read
    /// here.
    seq: i64,
    /// The destination's post-publish stat, gathered by the caller.
    stat: &'a StatFacts,
    /// The caller's own bracketed-stat verdict around `stat` — whether a
    /// racer landing between the publish and the stat can be ruled out.
    confirmed: bool,
    /// The disk-side observation this commit reconciled against, when one
    /// exists — a swap-race's displaced-bytes capture (`record_fresh_from_stat`'s
    /// own `Observation`, already recorded before this commit runs).
    /// `None` on a plain `Committed` outcome, which reconciled against
    /// nothing.
    reconciled: Option<ObsId>,
}

/// ONE tx — blob put (hash of the bytes actually written) + observation
/// (`origin='save'`) + `saved_obs` update + re-Bind (path/inode/device/
/// `kind='file'`, caller-supplied post-swap stat). No `vfs` call: every
/// disk-sourced fact `facts` carries was already gathered by the caller on
/// its own thread, before this op was ever enqueued (I1's "no DB tx across
/// a vfs call" contract, now trivially true — this function makes no vfs
/// call at all). The blob put and the observation that references its hash
/// used to be two separate transactions — a cross-process GC sweep landing
/// between them could delete the blob before the reference committed,
/// failing the reference with no retry ([rune-db 2]); both commit
/// atomically here, following the pattern `snapshot::create_snapshot`
/// already uses.
fn commit_save_from_stat(
    conn: &mut Connection,
    ds: DocSession,
    facts: CommitFacts<'_>,
    now: SystemTime,
) -> Result<Observation, Error> {
    let at = crate::session::format_rfc3339_nanos(now);
    let resolved_str = facts.resolved_path.to_string();

    retry::with_retry(conn, |tx| {
        let hash = crate::blob::put_blob(tx, facts.data)?;

        let obs = crate::adopt::record_adoption_tx(
            tx,
            ds.doc_id,
            ds.session_id,
            ObservationMeta {
                blob_hash: &hash,
                seq: Some(facts.seq),
                origin: ObsOrigin::Save,
                confirmed: Confirmation::from_bracket(facts.confirmed),
            },
            facts.stat,
            &at,
            facts.reconciled,
        )?;

        rebind_document_tx(
            tx,
            ds.doc_id,
            Rebind {
                path: &resolved_str,
                stat: facts.stat,
                at: &at,
            },
        )?;

        Ok(obs)
    })
}

// Kept in a sibling file: this module's own CAS bookkeeping stays under the
// 500-line budget on its own merits.
#[cfg(test)]
#[path = "materialize_tests.rs"]
mod tests;
