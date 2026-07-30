//! `Materialize`'s data model — `DocSession`, the CAS decision/outcome
//! types [`prepare_materialize`]/[`record_materialize_outcome`] exchange
//! with the caller. Split out of `materialize.rs` (§1.6) and re-exported
//! from there, so this stays purely a data-model file with no logic.
//!
//! [`prepare_materialize`]: crate::materialize::prepare_materialize
//! [`record_materialize_outcome`]: crate::materialize::record_materialize_outcome

use crate::observation::{Observation, StatFacts};

/// `doc_id`/`session_id` bundled together — every materialize operation
/// needs both, and threading them as a pair (rather than two separate
/// parameters at every call site) is what keeps each signature under
/// clippy's argument-count lint without an `#[allow]` (repo rule: no such
/// allow outside test code).
#[derive(Clone, Copy, Debug)]
pub struct DocSession {
    pub doc_id: i64,
    pub session_id: i64,
}

/// The outcome of a materialize attempt, assembled by
/// `record_materialize_outcome` (`missing` is set directly by the caller
/// instead — see `save.rs`'s dance): `Missing`/`Fresh`-on-refusal/`Raced`
/// stay mutually exclusive discriminants, never a shared sentinel
/// (this crate's "Options for absent facts" rule).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MatResult {
    pub committed: bool,
    /// Meaningful when `committed` (an ordinary save OR a `raced` win).
    pub saved: Option<Observation>,
    /// Meaningful when `!committed && !missing`, OR `raced` (the
    /// displaced/conflicting observation).
    pub fresh: Option<Observation>,
    /// `true` when `!committed` because the target doesn't exist and
    /// `bind_new` was `false` (§1.4.4 — never silently (re)create). The
    /// caller decides this entirely on its own vfs read; it is never
    /// constructed by anything in this module.
    pub missing: bool,
    /// `true` when `committed` via a swap-race (F5): a writer raced inside
    /// the atomic-swap window the caller performed, so the displaced bytes
    /// differ from `expect`, but OUR bytes are already physically at the
    /// target — this write commits for real, and the raced writer's
    /// displaced bytes are ALSO surfaced (`fresh`, `origin='swap'`).
    pub raced: bool,
}

/// Bookkeeping-only decision data `prepare_materialize` hands the caller
/// before any `vfs` call happens — no `vfs` call is made to produce this,
/// only DB reads. `Default` (both fields empty) is exactly what `bind_new`
/// needs: there is no bound path to disagree with, and `materialize_create`
/// never consulted `expect` in the first place.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaterializePrep {
    /// `documents.path`, unresolved — the caller resolves both this and its
    /// own target through its own `vfs` and refuses on disagreement (a
    /// caller bug, not an ordinary CAS race — was `materialize`'s own
    /// path-parameter check, [rune-db 5]). `None` when `bind_new` (nothing
    /// to disagree with).
    pub bound_path: Option<String>,
    /// `expect`'s `blob_hash` — the CAS baseline the caller compares the
    /// live target's hash against before writing. Empty when `bind_new`
    /// (the create path never had a CAS baseline to compare).
    pub expect_hash: String,
}

/// What the caller's own `vfs` work concluded, carrying every disk-sourced
/// fact `record_materialize_outcome` needs — `materialize.rs` never calls
/// `vfs` itself to re-derive any of it. A target that turned out `missing`
/// (bind_new=false, `NotFound`) or a genuine I/O failure never reach this
/// type at all: neither one has anything for the DB to record (see
/// `save.rs`'s caller-side dance for both).
pub enum MaterializeOutcome {
    /// The live target's hash disagreed with `expect` (an ordinary CAS
    /// refusal), or a concurrent creator won a `bind_new` race — no write
    /// was attempted; `data`/`stat` describe whatever is actually on disk
    /// now.
    Conflict {
        data: Vec<u8>,
        origin: &'static str,
        stat: StatFacts,
    },
    /// The write committed with no race.
    Committed { data: Vec<u8>, stat: StatFacts },
    /// The write committed AND a racer's displaced bytes were captured in
    /// the same atomic-swap window (F5).
    Raced {
        data: Vec<u8>,
        stat: StatFacts,
        displaced: Vec<u8>,
        displaced_stat: StatFacts,
    },
}
