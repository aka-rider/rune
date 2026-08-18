//! `Replica`: the state machine tracking whether, and how, a `Document`'s
//! local journal is mirrored to the recovery store. Restores the 1:1
//! correspondence between local journal positions and durable `events`
//! rows that a plain `Option<DocDb>` could not express: an edit typed
//! while a `Load`/`CreateScratch` op is still in flight has nowhere durable
//! to go yet, but must not be lost either — `Binding` buffers it as a
//! `ReplicaStep` until the ack installs the real `DocDb`, at which point
//! every buffered step replays as a real `AppendEdit`, in order, before the
//! document is considered `Bound`.

use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;

use crate::db::DocDb;

/// One buffered edit batch, captured with exactly the arguments
/// `db_enqueue::append_edit` would otherwise have sent to the store —
/// replayed verbatim, in order, once the document's `DocDb` installs.
pub(crate) struct ReplicaStep {
    pub edits: Vec<AppliedEdit>,
    pub cursors_before: Vec<Cursor>,
    pub cursors_after: Vec<Cursor>,
}

impl ReplicaStep {
    pub(crate) fn new(
        edits: &[AppliedEdit],
        cursors_before: &[Cursor],
        cursors_after: &[Cursor],
    ) -> ReplicaStep {
        ReplicaStep {
            edits: edits.to_vec(),
            cursors_before: cursors_before.to_vec(),
            cursors_after: cursors_after.to_vec(),
        }
    }
}

/// A `Document`'s relationship to the app-wide recovery store.
pub(crate) enum Replica {
    /// No recovery journal for this document — an ephemeral/help document,
    /// one opened with no store or a degraded one, or one whose `Load`/
    /// `CreateScratch` ack was refused.
    Detached,
    /// A `Load`/`CreateScratch` op is in flight; every edit committed in
    /// the meantime is buffered here rather than dropped, in commit order.
    /// `base` is the buffer content the window opened on — the content the
    /// first buffered step's coordinates assume, which the install compares
    /// against what the bound row's journal actually reconstructs to.
    Binding {
        base: String,
        pending: Vec<ReplicaStep>,
    },
    /// This document's row is installed; every edit reaches the store
    /// directly.
    Bound(DocDb),
}

impl Replica {
    pub(crate) fn is_bound(&self) -> bool {
        matches!(self, Replica::Bound(_))
    }

    pub(crate) fn doc_db(&self) -> Option<&DocDb> {
        match self {
            Replica::Bound(db) => Some(db),
            Replica::Detached | Replica::Binding { .. } => None,
        }
    }

    pub(crate) fn doc_db_mut(&mut self) -> Option<&mut DocDb> {
        match self {
            Replica::Bound(db) => Some(db),
            Replica::Detached | Replica::Binding { .. } => None,
        }
    }

    /// Takes whatever `Binding` window this replica buffered — its base
    /// content and every step committed since — leaving it `Detached`: the
    /// shared first half of installing a fresh `DocDb` once a `Load`/
    /// `CreateScratch` ack lands, before the caller replaces it with
    /// `Bound`. A `Detached` or `Bound` replica yields an empty window
    /// (`base: None`, no steps).
    pub(crate) fn take_window(&mut self) -> ReplicaWindow {
        match std::mem::replace(self, Replica::Detached) {
            Replica::Binding { base, pending } => ReplicaWindow {
                base: Some(base),
                pending,
            },
            Replica::Detached | Replica::Bound(_) => ReplicaWindow {
                base: None,
                pending: Vec::new(),
            },
        }
    }
}

/// What [`Replica::take_window`] hands the install: the content the window
/// opened on (when there was a window at all) and the steps it buffered.
pub(crate) struct ReplicaWindow {
    pub base: Option<String>,
    pub pending: Vec<ReplicaStep>,
}
