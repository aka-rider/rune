use rune_core::buffer::AppliedEdit;
use rune_core::cursor::Cursor;

use crate::db::DocDb;

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

pub(crate) enum Replica {
    Detached,
    Binding {
        base: String,
        pending: Vec<ReplicaStep>,
    },
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

pub(crate) struct ReplicaWindow {
    pub base: Option<String>,
    pub pending: Vec<ReplicaStep>,
}
