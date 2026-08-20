use rune_db::{ObsId, Seq};

use crate::db::PublishMode;

pub struct DocDb {
    pub db_id: i64,
    pub publish_mode: PublishMode,
    pub last_known_seq: Seq,
    pub snapshot_generation: u32,
    pub(crate) undo_offset: i64,
    pub(crate) undo_floor: i64,
    pub(crate) appends_sent: i64,
    pub(crate) pending_rebase: Option<crate::document::ReplicaStep>,
}

impl DocDb {
    pub fn new(db_id: i64, publish_mode: PublishMode, last_known_seq: Seq) -> DocDb {
        DocDb {
            db_id,
            publish_mode,
            last_known_seq,
            snapshot_generation: 0,
            undo_offset: 0,
            undo_floor: 0,
            appends_sent: 0,
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
}

impl FileBinding {
    pub fn new(expect_obs: Option<ObsId>) -> FileBinding {
        FileBinding {
            expect_obs,
            pending_rebaseline_hash: None,
            baseline_epoch: 0,
            pending_probe: false,
        }
    }
}
