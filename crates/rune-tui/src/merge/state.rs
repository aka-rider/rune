//! Merge mode's own state shape (plan WP3.S3): a plain field on `App`,
//! mirroring `App.rename`'s own typed-machine-not-a-boolean precedent.
//! Nothing here runs I/O or touches the buffer — [`super::begin`]/
//! [`super::exit_in_place`] are the writers.

use rune_db::ObsId;

use crate::document::DocumentId;

/// Why this merge attempt was started (plan Assumption A2 — shared entry
/// pipeline): `Merge` installs the 3-way merge result; `Discard` (a future
/// work package's guard `[D]iscard` answer) installs the fresh disk bytes
/// outright. Carried through `Pending` so the `MergePrep` landing knows
/// which of the two to do once the fresh bytes arrive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeIntent {
    Merge,
    Discard,
}

/// One still-unresolved conflict's ORIGINAL ours/theirs text, aligned
/// index-for-index with `blocks` below — immutable for the lifetime of the
/// merge attempt, unlike `Block`'s byte range (which shifts as earlier
/// blocks in the buffer get resolved to a different length).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Conflict {
    pub ours: String,
    pub theirs: String,
}

/// One conflict's current byte range in the LIVE working-form buffer —
/// `[start, end)`, a half-open span covering the whole framed block
/// (`<<<<<<< editor` through `>>>>>>> disk\n` inclusive) while `resolved`
/// is `false`, or the collapsed replacement span once a future resolver
/// (plan WP4) accepts it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Block {
    pub start: usize,
    pub end: usize,
    pub resolved: bool,
}

/// Merge mode's state machine (plan WP3.S3). `Default` is `Inactive` — no
/// merge attempt is ever implicitly in flight.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum MergeState {
    #[default]
    Inactive,
    /// A `MergePrep` op is in flight for `doc` — `generation` is this attempt's own
    /// ticket (`App::next_merge_gen`'s value at `begin` time), so a LATER
    /// `^M` (superseding this attempt before it lands) can't have its
    /// eventual stale ack mistaken for the current one.
    Pending {
        doc: DocumentId,
        generation: u32,
        intent: MergeIntent,
    },
    /// The working-form buffer is installed and merge mode owns the editor
    /// view. `saved_display_name` is `doc`'s `display_name` from BEFORE
    /// entry (`None` for an ordinary path-derived document) — restored
    /// verbatim by `exit_in_place`.
    Active {
        doc: DocumentId,
        conflicts: Vec<Conflict>,
        blocks: Vec<Block>,
        cur: usize,
        saved_display_name: Option<String>,
        /// The disk observation the working form was built against — the
        /// save-CAS baseline `exit_in_place` advances to ONLY when the
        /// merge completes (every block resolved). An unresolved
        /// retirement never advances it: the buffer still holds conflict
        /// markers the disk never agreed to, and a ⌘S must keep
        /// CAS-refusing against the external bytes until the user
        /// actually resolves or force-saves through the guard.
        theirs_obs: ObsId,
    },
}

impl MergeState {
    /// The document this attempt concerns, in any non-`Inactive` state —
    /// `None` only for `Inactive`, which concerns no document at all.
    pub fn doc(&self) -> Option<DocumentId> {
        match self {
            MergeState::Inactive => None,
            MergeState::Pending { doc, .. } | MergeState::Active { doc, .. } => Some(*doc),
        }
    }

    /// How many blocks in an `Active` state are still unresolved — `0` for
    /// every other state, matching `blocks.iter().filter(...)`'s own empty
    /// sum for a merge that was never entered.
    pub fn unresolved_count(&self) -> usize {
        match self {
            MergeState::Active { blocks, .. } => blocks.iter().filter(|b| !b.resolved).count(),
            _ => 0,
        }
    }
}
