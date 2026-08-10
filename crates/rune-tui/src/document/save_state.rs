use std::sync::Arc;

use rune_core::assert_invariant;

#[derive(Clone)]
pub(crate) struct SaveCapture {
    pub(crate) version: u64,
    pub(crate) content: Arc<str>,
}

#[derive(Default)]
pub(crate) enum SaveState {
    #[default]
    Idle,
    Direct {
        capture: SaveCapture,
    },
}

impl SaveState {
    pub(crate) fn is_idle(&self) -> bool {
        matches!(self, SaveState::Idle)
    }

    pub(crate) fn pending_version(&self) -> Option<u64> {
        match self {
            SaveState::Idle => None,
            SaveState::Direct { capture } => Some(capture.version),
        }
    }

    pub(crate) fn begin_direct(&mut self, version: u64, content: Arc<str>) {
        assert_invariant!(self.is_idle(), || "begin_direct requires an idle SaveState");
        *self = SaveState::Direct {
            capture: SaveCapture { version, content },
        };
    }

    pub(crate) fn resolve(&mut self) -> Option<SaveCapture> {
        match std::mem::take(self) {
            SaveState::Idle => None,
            SaveState::Direct { capture } => Some(capture),
        }
    }
}
