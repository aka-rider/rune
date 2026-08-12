//! `Materialize`'s data model — `DocSession`, the CAS decision/outcome
//! types [`prepare_materialize`]/[`record_materialize_outcome`] exchange
//! with the caller — purely a data-model module, no logic.
//!
//! [`prepare_materialize`]: crate::materialize::prepare_materialize
//! [`record_materialize_outcome`]: crate::materialize::record_materialize_outcome

use crate::ids::{BlobHash, DocId, ObsId, SessionId};
use crate::obs_origin::ObsOrigin;
use crate::observation::{Observation, StatFacts};
use crate::sync::SyncKind;

/// `doc_id`/`session_id` bundled together — every materialize operation
/// needs both, and threading them as a pair (rather than two separate
/// parameters at every call site) is what keeps each signature under
/// clippy's argument-count lint without an `#[allow]` (repo rule: no such
/// allow outside test code).
#[derive(Clone, Copy, Debug)]
pub struct DocSession {
    pub doc_id: DocId,
    pub session_id: SessionId,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MatResult {
    Committed {
        saved: Option<Observation>,
    },
    CommittedRaced {
        saved: Observation,
        displaced: Box<Observation>,
    },
    Refused {
        fresh: Observation,
    },
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterializeTarget {
    BindNew,
    Existing { expect: ObsId },
}

#[derive(Clone, Debug, PartialEq)]
pub enum MaterializePrep {
    Create,
    Overwrite {
        bound_path: String,
        expect_hash: BlobHash,
        sync: SyncKind,
    },
}

/// What the caller's own `vfs` work concluded, carrying every disk-sourced
/// fact `record_materialize_outcome` needs — this crate never calls `vfs`
/// itself to re-derive any of it. A target that turned out `missing`
/// (`Existing`, `NotFound`) or a genuine I/O failure never reach this
/// type at all: neither one has anything for the DB to record.
pub enum MaterializeOutcome {
    /// The live target's hash disagreed with `expect` (an ordinary CAS
    /// refusal), or a concurrent creator won a `BindNew` race — no write
    /// was attempted; `data`/`stat` describe whatever is actually on disk
    /// now. `confirmed` carries the caller's own bracketed-read verdict
    /// (stat-read-stat around this exact `data`) — a read caught
    /// mid-external-rewrite must never masquerade as a stable fact.
    Conflict {
        data: Vec<u8>,
        origin: ObsOrigin,
        stat: StatFacts,
        confirmed: bool,
    },
    /// The write committed with no race. `confirmed` carries the caller's
    /// own bracketed-stat verdict around the post-publish `stat` — a racer
    /// landing between the publish and the stat must never let this
    /// observation's blob and stat quietly disagree.
    Committed {
        data: Vec<u8>,
        stat: StatFacts,
        confirmed: bool,
    },
    /// The write committed AND a racer's displaced bytes were captured in
    /// the same atomic-swap window (F5). `confirmed` describes `stat` only —
    /// `displaced`/`displaced_stat` are a one-shot read of the caller's own
    /// private temp file, never contended, so bracketing buys nothing there.
    Raced {
        data: Vec<u8>,
        stat: StatFacts,
        confirmed: bool,
        displaced: Vec<u8>,
        displaced_stat: StatFacts,
    },
}
