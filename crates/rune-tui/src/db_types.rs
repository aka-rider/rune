use rune_db::{BindingToken, ObsId, Seq};

use crate::db::PublishMode;

pub struct DocDb {
    pub db_id: i64,
    pub publish_mode: PublishMode,
    pub last_known_seq: Seq,
    pub snapshot_generation: u32,
    /// This binding's own key into the writer thread's `DocUndoState` map —
    /// minted fresh every time a `Document` binds or rebinds, so two
    /// bindings sharing one `db_id` (unreachable via any real open path,
    /// but not structurally prevented) each get an independent numbering
    /// instead of racing to fill one shared sequence.
    pub(crate) token: BindingToken,
    /// The durable seq `token`'s local position `0` resolves to — frozen at
    /// whatever `last_known_seq` was when `token` was minted, carried on
    /// every op that might be `token`'s first sighting at the writer.
    pub(crate) token_base_seq: Seq,
    /// `local_pos - undo_offset` is the position sent to the writer as
    /// `token`'s own local position; `undo_floor` is the smallest resolved
    /// position `token` can answer exactly (a deep undo below it re-anchors
    /// via `db_enqueue::rebase_move` instead). Both are fixed the instant
    /// `token` is minted and never grow — see `db_ack::bind_document_row`.
    pub(crate) undo_offset: i64,
    pub(crate) undo_floor: i64,
    /// Whether this binding has ever had to send an edit whose coordinates
    /// were computed against a buffer that no longer matches what the row
    /// actually durably reconstructs to (`db_enqueue::resolve_drift`'s own
    /// doc comment) — sticky once true: a token that has diverged never
    /// trusts its own raw local coordinates again, only a fresh bind does.
    pub(crate) diverged: bool,
    /// What THIS binding believes the row's actual content is, right now —
    /// seeded from the row's true content at bind time, advanced by every
    /// edit this binding sends. Compared against the shared
    /// `FileBinding::shared_content` to detect drift a SIBLING binding (or
    /// an external reload this binding never saw) introduced.
    pub(crate) synced_content: String,
    pub(crate) pending_rebase: Option<crate::document::ReplicaStep>,
}

impl DocDb {
    pub fn new(db_id: i64, publish_mode: PublishMode, last_known_seq: Seq) -> DocDb {
        DocDb {
            db_id,
            publish_mode,
            last_known_seq,
            snapshot_generation: 0,
            token: BindingToken::next(),
            token_base_seq: last_known_seq,
            undo_offset: 0,
            undo_floor: 0,
            diverged: false,
            synced_content: String::new(),
            pending_rebase: None,
        }
    }

    pub(crate) fn resolve_append_ack(&mut self, seq: Seq) {
        self.last_known_seq = self.last_known_seq.max(seq);
    }
}

pub struct FileBinding {
    pub expect_obs: Option<ObsId>,
    pub pending_rebaseline_hash: Option<String>,
    pub baseline_epoch: u32,
    pub pending_probe: bool,
    /// The row's actual current content, as best known from every bind and
    /// every successful append any binding on this `db_id` has sent — the
    /// chokepoint `db_enqueue::resolve_drift` compares a binding's own
    /// `DocDb::synced_content` against to detect a sibling's (or an earlier
    /// reload's) drift. Empty until the first bind or edit populates it.
    pub(crate) shared_content: String,
}

impl FileBinding {
    pub fn new(expect_obs: Option<ObsId>) -> FileBinding {
        FileBinding {
            expect_obs,
            pending_rebaseline_hash: None,
            baseline_epoch: 0,
            pending_probe: false,
            shared_content: String::new(),
        }
    }
}
