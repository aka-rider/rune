use std::path::PathBuf;
use std::sync::Arc;

use rune_core::assert_invariant;

use crate::save::SaveMode;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SaveTicket(u64);

#[derive(Clone, Debug, PartialEq)]
pub struct SaveCapture {
    pub(crate) version: u64,
    pub(crate) content: Arc<str>,
}

#[derive(Clone)]
pub(crate) struct PublishParams {
    pub(crate) path: PathBuf,
    pub(crate) publish_mode: crate::db::PublishMode,
    pub(crate) db_id: i64,
    pub(crate) seq: i64,
    pub(crate) mode: SaveMode,
    pub(crate) bind_target: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SavePhase {
    Idle,
    Direct,
    Preparing,
    Publishing,
    Recording { published: bool },
}

#[derive(Default)]
pub(crate) enum SaveState {
    #[default]
    Idle,
    Direct {
        ticket: SaveTicket,
        capture: SaveCapture,
    },
    Preparing {
        ticket: SaveTicket,
        capture: SaveCapture,
        params: PublishParams,
        prep_op: u64,
    },
    Publishing {
        ticket: SaveTicket,
        capture: SaveCapture,
        params: PublishParams,
    },
    Recording {
        ticket: SaveTicket,
        capture: SaveCapture,
        record_op: u64,
        published: bool,
        bind_target: Option<PathBuf>,
    },
}

impl SaveState {
    pub(crate) fn is_idle(&self) -> bool {
        matches!(self, SaveState::Idle)
    }

    pub(crate) fn is_publishing(&self) -> bool {
        matches!(self, SaveState::Publishing { .. })
    }

    pub(crate) fn phase(&self) -> SavePhase {
        match self {
            SaveState::Idle => SavePhase::Idle,
            SaveState::Direct { .. } => SavePhase::Direct,
            SaveState::Preparing { .. } => SavePhase::Preparing,
            SaveState::Publishing { .. } => SavePhase::Publishing,
            SaveState::Recording { published, .. } => SavePhase::Recording {
                published: *published,
            },
        }
    }

    fn capture(&self) -> Option<&SaveCapture> {
        match self {
            SaveState::Idle => None,
            SaveState::Direct { capture, .. }
            | SaveState::Preparing { capture, .. }
            | SaveState::Publishing { capture, .. }
            | SaveState::Recording { capture, .. } => Some(capture),
        }
    }

    pub(crate) fn pending_version(&self) -> Option<u64> {
        self.capture().map(|c| c.version)
    }

    pub(crate) fn ticket(&self) -> Option<SaveTicket> {
        match self {
            SaveState::Idle => None,
            SaveState::Direct { ticket, .. }
            | SaveState::Preparing { ticket, .. }
            | SaveState::Publishing { ticket, .. }
            | SaveState::Recording { ticket, .. } => Some(*ticket),
        }
    }

    pub(crate) fn begin_direct(&mut self, ticket: SaveTicket, version: u64, content: Arc<str>) {
        assert_invariant!(self.is_idle(), || "begin_direct requires an idle SaveState");
        *self = SaveState::Direct {
            ticket,
            capture: SaveCapture { version, content },
        };
    }

    pub(crate) fn begin_prepare(
        &mut self,
        ticket: SaveTicket,
        version: u64,
        content: Arc<str>,
        params: PublishParams,
        prep_op: u64,
    ) {
        assert_invariant!(
            self.is_idle(),
            || "begin_prepare requires an idle SaveState"
        );
        *self = SaveState::Preparing {
            ticket,
            capture: SaveCapture { version, content },
            params,
            prep_op,
        };
    }

    pub(crate) fn prep_op(&self) -> Option<u64> {
        match self {
            SaveState::Preparing { prep_op, .. } => Some(*prep_op),
            _ => None,
        }
    }

    pub(crate) fn preparing_mode(&self) -> Option<SaveMode> {
        match self {
            SaveState::Preparing { params, .. } => Some(params.mode),
            _ => None,
        }
    }

    pub(crate) fn advance_to_publishing(
        &mut self,
    ) -> Option<(SaveTicket, SaveCapture, PublishParams)> {
        match std::mem::take(self) {
            SaveState::Preparing {
                ticket,
                capture,
                params,
                ..
            } => {
                *self = SaveState::Publishing {
                    ticket,
                    capture: capture.clone(),
                    params: params.clone(),
                };
                Some((ticket, capture, params))
            }
            other => {
                *self = other;
                None
            }
        }
    }

    #[must_use]
    pub(crate) fn advance_to_recording(&mut self, record_op: u64, published: bool) -> bool {
        match std::mem::take(self) {
            SaveState::Publishing {
                ticket,
                capture,
                params,
            } => {
                *self = SaveState::Recording {
                    ticket,
                    capture,
                    record_op,
                    published,
                    bind_target: params.bind_target,
                };
                true
            }
            other => {
                *self = other;
                false
            }
        }
    }

    pub(crate) fn record_op(&self) -> Option<u64> {
        match self {
            SaveState::Recording { record_op, .. } => Some(*record_op),
            _ => None,
        }
    }

    pub(crate) fn take_bind_target(&mut self) -> Option<PathBuf> {
        match self {
            SaveState::Preparing { params, .. } | SaveState::Publishing { params, .. } => {
                params.bind_target.take()
            }
            SaveState::Recording { bind_target, .. } => bind_target.take(),
            _ => None,
        }
    }

    pub(crate) fn bind_target(&self) -> Option<&PathBuf> {
        match self {
            SaveState::Preparing { params, .. } | SaveState::Publishing { params, .. } => {
                params.bind_target.as_ref()
            }
            SaveState::Recording { bind_target, .. } => bind_target.as_ref(),
            _ => None,
        }
    }

    pub(crate) fn resolve(&mut self) -> Option<SaveCapture> {
        match std::mem::take(self) {
            SaveState::Idle => None,
            SaveState::Direct { capture, .. }
            | SaveState::Preparing { capture, .. }
            | SaveState::Publishing { capture, .. }
            | SaveState::Recording { capture, .. } => Some(capture),
        }
    }
}

#[derive(Default)]
pub(crate) struct SaveTicketMint(u64);

impl SaveTicketMint {
    pub(crate) fn mint(&mut self) -> SaveTicket {
        self.0 = self.0.wrapping_add(1);
        SaveTicket(self.0)
    }
}
