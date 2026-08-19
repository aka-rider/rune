use crate::document::DocumentId;
use crate::generation::MergeGen as Generation;

use super::session::MergeSession;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeIntent {
    Merge,
    Discard,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum MergeState {
    #[default]
    Inactive,
    Pending {
        doc: DocumentId,
        generation: Generation,
        intent: MergeIntent,
    },
    Active {
        doc: DocumentId,
        session: MergeSession,
    },
}

impl MergeState {
    pub fn doc(&self) -> Option<DocumentId> {
        match self {
            MergeState::Inactive => None,
            MergeState::Pending { doc, .. } | MergeState::Active { doc, .. } => Some(*doc),
        }
    }

    pub fn unresolved_count(&self) -> usize {
        match self {
            MergeState::Active { session, .. } => session.unresolved_count(),
            _ => 0,
        }
    }
}
