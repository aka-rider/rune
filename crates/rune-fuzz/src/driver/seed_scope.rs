use rune_tui::document::DocumentId;

use crate::step::MsgTag;

pub(super) fn tag_delivers_seed_save(tag: &MsgTag, seed_doc: DocumentId) -> bool {
    match tag {
        MsgTag::SaveDone { id, ok: true, .. } => *id == seed_doc,
        MsgTag::Db {
            doc,
            save_committed: true,
            ..
        } => *doc == Some(seed_doc),
        _ => false,
    }
}

pub(super) fn tag_publishes_seed_doc(tag: &MsgTag, seed_doc: DocumentId) -> bool {
    match tag {
        MsgTag::SaveDone { id, ok: true, .. } => *id == seed_doc,
        MsgTag::MaterializeVfsDone {
            id,
            committed: true,
        } => *id == seed_doc,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_tui::app::App;
    use rune_vfs::Vfs;
    use std::sync::Arc;

    fn seed_and_foreign_doc_ids() -> (DocumentId, DocumentId) {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(rune_vfs::Mem::new());
        let mut app = App::new(Buffer::new(""), None, vfs, None);
        let seed = app.active;
        let foreign = app.open_document(Buffer::new(""));
        (seed, foreign)
    }

    #[test]
    fn tag_delivers_seed_save_accepts_the_seed_docs_own_save_done() {
        let (seed, _foreign) = seed_and_foreign_doc_ids();
        let tag = MsgTag::SaveDone {
            id: seed,
            version: 1,
            ok: true,
        };
        assert!(tag_delivers_seed_save(&tag, seed));
    }

    #[test]
    fn tag_delivers_seed_save_rejects_a_foreign_docs_save_done() {
        let (seed, foreign) = seed_and_foreign_doc_ids();
        let tag = MsgTag::SaveDone {
            id: foreign,
            version: 1,
            ok: true,
        };
        assert!(!tag_delivers_seed_save(&tag, seed));
    }

    #[test]
    fn tag_delivers_seed_save_accepts_a_db_commit_naming_the_seed_doc() {
        let (seed, _foreign) = seed_and_foreign_doc_ids();
        let tag = MsgTag::Db {
            op_id: 1,
            doc: Some(seed),
            save_committed: true,
        };
        assert!(tag_delivers_seed_save(&tag, seed));
    }

    #[test]
    fn tag_delivers_seed_save_rejects_a_db_commit_naming_a_foreign_doc() {
        let (seed, foreign) = seed_and_foreign_doc_ids();
        let tag = MsgTag::Db {
            op_id: 1,
            doc: Some(foreign),
            save_committed: true,
        };
        assert!(!tag_delivers_seed_save(&tag, seed));
    }

    #[test]
    fn tag_delivers_seed_save_rejects_a_db_commit_naming_no_doc() {
        let (seed, _foreign) = seed_and_foreign_doc_ids();
        let tag = MsgTag::Db {
            op_id: 1,
            doc: None,
            save_committed: true,
        };
        assert!(!tag_delivers_seed_save(&tag, seed));
    }

    #[test]
    fn tag_publishes_seed_doc_accepts_a_committed_materialize_vfs_done_for_the_seed_doc() {
        let (seed, _foreign) = seed_and_foreign_doc_ids();
        let tag = MsgTag::MaterializeVfsDone {
            id: seed,
            committed: true,
        };
        assert!(tag_publishes_seed_doc(&tag, seed));
    }

    #[test]
    fn tag_publishes_seed_doc_rejects_a_committed_materialize_vfs_done_for_a_foreign_doc() {
        let (seed, foreign) = seed_and_foreign_doc_ids();
        let tag = MsgTag::MaterializeVfsDone {
            id: foreign,
            committed: true,
        };
        assert!(!tag_publishes_seed_doc(&tag, seed));
    }

    #[test]
    fn tag_publishes_seed_doc_rejects_an_uncommitted_materialize_vfs_done_for_the_seed_doc() {
        let (seed, _foreign) = seed_and_foreign_doc_ids();
        let tag = MsgTag::MaterializeVfsDone {
            id: seed,
            committed: false,
        };
        assert!(!tag_publishes_seed_doc(&tag, seed));
    }
}
