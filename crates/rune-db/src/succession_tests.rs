#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, params};

use rune_core::buffer::AppliedEdit;
use rune_core::undo::EditKind;
use rune_vfs::{Mem, VfsTestExt};

use crate::ids::{DocId, Seq, SessionId};
use crate::journal::{append_edit, current_seq, move_undo_pos};
use crate::journal_append::EditBatch;
use crate::load::{LoadResult, load};
use crate::obs_origin::ObsOrigin;
use crate::sync::SyncKind;
use crate::test_support::{always_dead, open};
use crate::{adopt, confirmation::Confirmation, observation, retry};

const PATH: &str = "/doc.md";

struct World {
    conn: Connection,
    vfs: Mem,
}

fn alive(_pid: i64, _started_at: &str) -> bool {
    true
}

impl World {
    fn new(disk: &str) -> World {
        let world = World {
            conn: open(),
            vfs: Mem::new(),
        };
        world
            .vfs
            .save_atomic(Path::new(PATH), disk.as_bytes())
            .expect("seed disk");
        world
    }

    fn session(&self) -> SessionId {
        crate::session::establish_session(&self.conn, SystemTime::now()).expect("session")
    }

    fn load(&mut self, session: SessionId, liveness: &dyn Fn(i64, &str) -> bool) -> LoadResult {
        load(
            &mut self.conn,
            &self.vfs,
            session,
            liveness,
            Path::new(PATH),
            SystemTime::now(),
        )
        .expect("load")
    }

    fn type_at_start(&mut self, session: SessionId, doc: DocId, text: &str) -> Seq {
        let tx = self.conn.transaction().expect("tx");
        let seq = append_edit(
            &tx,
            session,
            SystemTime::now(),
            doc,
            EditBatch {
                edits: &[AppliedEdit {
                    start: 0,
                    end: 0,
                    deleted: String::new(),
                    insert: text.to_string(),
                }],
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Insert,
            },
        )
        .expect("append_edit");
        tx.commit().expect("commit");
        seq
    }

    fn undo_to(&mut self, session: SessionId, doc: DocId, position: Seq) {
        let tx = self.conn.transaction().expect("tx");
        move_undo_pos(&tx, session, doc, position).expect("move_undo_pos");
        tx.commit().expect("commit");
    }

    fn save(&mut self, session: SessionId, doc: DocId, content: &str) {
        self.vfs
            .save_atomic(Path::new(PATH), content.as_bytes())
            .expect("save");
        let seq = self.position(session, doc);
        let hash = retry::with_retry(&mut self.conn, |tx| {
            crate::blob::put_blob(tx, content.as_bytes())
        })
        .expect("put_blob");
        adopt::record_adoption(
            &mut self.conn,
            doc,
            session,
            observation::ObservationMeta {
                blob_hash: &hash,
                seq: Some(seq.0),
                origin: ObsOrigin::Save,
                confirmed: Confirmation::Confirmed,
            },
            &observation::StatFacts::default(),
            SystemTime::now(),
            None,
        )
        .expect("record_adoption");
    }

    fn external_rewrite(&self, content: &str) {
        self.vfs
            .save_atomic(Path::new(PATH), content.as_bytes())
            .expect("rewrite");
    }

    fn position(&mut self, session: SessionId, doc: DocId) -> Seq {
        retry::with_retry(&mut self.conn, |tx| current_seq(tx, session, doc)).expect("current_seq")
    }

    fn events_of(&self, session: SessionId) -> i64 {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=?1",
                params![session],
                |r| r.get(0),
            )
            .expect("count")
    }
}

fn done_seqs(result: &LoadResult) -> Vec<Seq> {
    result.history.done.iter().map(|s| s.seq).collect()
}

#[test]
fn a_clean_dead_session_is_adopted_with_its_redo_tail_and_undo_position() {
    let mut w = World::new("one");
    let a = w.session();
    let doc = w.load(a, &alive).doc_id;
    let first = w.type_at_start(a, doc, "A");
    w.save(a, doc, "Aone");
    let second = w.type_at_start(a, doc, "B");
    w.undo_to(a, doc, first);

    let b = w.session();
    let result = w.load(b, &always_dead);

    assert_eq!(result.recovered.content, "Aone");
    assert_eq!(done_seqs(&result), vec![first]);
    assert_eq!(
        result
            .history
            .undone
            .iter()
            .map(|s| s.seq)
            .collect::<Vec<_>>(),
        vec![second],
        "the redo tail must survive the restart"
    );
    assert_eq!(w.position(b, doc), first);
    assert_eq!(result.sync.kind, SyncKind::Clean);
    assert_eq!((w.events_of(a), w.events_of(b)), (0, 2));
}

#[test]
fn a_dead_session_with_an_unsaved_draft_is_adopted_step_by_step() {
    let mut w = World::new("base");
    let a = w.session();
    let doc = w.load(a, &alive).doc_id;
    let saved_step = w.type_at_start(a, doc, "x");
    w.save(a, doc, "xbase");
    let unsaved_step = w.type_at_start(a, doc, "y");

    let b = w.session();
    let result = w.load(b, &always_dead);

    assert_eq!(result.recovered.content, "yxbase");
    assert_eq!(done_seqs(&result), vec![saved_step, unsaved_step]);
    assert_eq!(result.sync.kind, SyncKind::BufferAhead);
}

#[test]
fn an_outside_change_after_the_last_save_is_one_more_undoable_step() {
    let mut w = World::new("base");
    let a = w.session();
    let doc = w.load(a, &alive).doc_id;
    let typed = w.type_at_start(a, doc, "A");
    w.save(a, doc, "Abase");
    w.external_rewrite("rewritten elsewhere");

    let b = w.session();
    let result = w.load(b, &always_dead);

    assert_eq!(result.recovered.content, "rewritten elsewhere");
    let seqs = done_seqs(&result);
    assert_eq!(seqs.len(), 2);
    assert_eq!(seqs[0], typed, "the earlier words stay reachable");
    let bridge = &result.history.done[1].step.edits;
    assert_eq!(
        (bridge[0].deleted.as_str(), bridge[0].insert.as_str()),
        ("Abase", "rewritten elsewhere")
    );
    assert_eq!(result.sync.kind, SyncKind::Clean);
    assert_eq!(w.position(b, doc), seqs[1]);
}

#[test]
fn an_unsaved_draft_survives_an_outside_change_with_its_history_attached() {
    let mut w = World::new("base");
    let a = w.session();
    let doc = w.load(a, &alive).doc_id;
    let unsaved_step = w.type_at_start(a, doc, "UNSAVED ");
    w.external_rewrite("rewritten elsewhere");

    let b = w.session();
    let result = w.load(b, &always_dead);

    assert_eq!(result.recovered.content, "UNSAVED base");
    assert_eq!(result.disk_content, "rewritten elsewhere");
    assert_eq!(done_seqs(&result), vec![unsaved_step]);
    assert_eq!(result.sync.kind, SyncKind::Diverged);
}

#[test]
fn a_live_most_recent_session_keeps_its_history() {
    let mut w = World::new("base");
    let a = w.session();
    let doc = w.load(a, &alive).doc_id;
    w.type_at_start(a, doc, "A");

    let b = w.session();
    let result = w.load(b, &alive);

    assert_eq!(result.recovered.content, "base");
    assert!(result.history.done.is_empty());
    assert_eq!((w.events_of(a), w.events_of(b)), (1, 0));
}

#[test]
fn an_unreplayable_dead_lineage_falls_back_to_inheriting_the_draft() {
    let mut w = World::new("hello");
    let a = w.session();
    let doc = w.load(a, &alive).doc_id;
    {
        let tx = w.conn.transaction().expect("tx");
        append_edit(
            &tx,
            a,
            SystemTime::now(),
            doc,
            EditBatch {
                edits: &[AppliedEdit {
                    start: 0,
                    end: 1,
                    deleted: "X".to_string(),
                    insert: String::new(),
                }],
                cursors_before: &[],
                cursors_after: &[],
                kind: EditKind::Other,
            },
        )
        .expect("append_edit");
        tx.commit().expect("commit");
    }

    let b = w.session();
    let result = w.load(b, &always_dead);

    assert_eq!(
        result.recovered.content, "ello",
        "the draft is still inherited"
    );
    assert_eq!(result.history.done.len(), 1, "as one bridge step");
    assert_eq!((w.events_of(a), w.events_of(b)), (1, 1));
}

impl World {
    fn snapshot(&mut self, session: SessionId, doc: DocId, content: &str, seq: Seq) {
        retry::with_retry(&mut self.conn, |tx| {
            crate::snapshot::create_snapshot(tx, session, SystemTime::now(), doc, content, seq)
                .map(drop)
        })
        .expect("create_snapshot");
    }
}

#[test]
fn a_dead_lineage_with_unreadable_old_rows_falls_back_to_inheriting_the_draft() {
    let mut w = World::new("one");
    let a = w.session();
    let doc = w.load(a, &alive).doc_id;
    let first = w.type_at_start(a, doc, "x");
    let second = w.type_at_start(a, doc, "y");
    w.snapshot(a, doc, "yxone", second);
    w.conn
        .execute("UPDATE events SET edits='{' WHERE seq=?1", params![first])
        .expect("corrupt a row below the newest snapshot");

    let b = w.session();
    let result = w.load(b, &always_dead);

    assert_eq!(result.recovered.content, "yxone");
    assert_eq!(result.history.done.len(), 1, "as one bridge step");
}

#[test]
fn adopting_a_dead_lineage_trims_it_to_the_cap_and_stays_recoverable_at_every_position() {
    let mut w = World::new("");
    let a = w.session();
    let doc = w.load(a, &alive).doc_id;
    let typed: String = ('a'..='z').cycle().take(1005).collect();
    for ch in typed.chars() {
        w.type_at_start(a, doc, &ch.to_string());
    }
    let reversed = |s: &str| s.chars().rev().collect::<String>();

    let b = w.session();
    let result = w.load(b, &always_dead);

    assert_eq!(result.recovered.content, reversed(&typed));
    assert_eq!(result.history.done.len(), 1000);
    assert_eq!(w.events_of(b), 1000, "the oldest five steps are trimmed");
    w.undo_to(b, doc, result.history.base_seq);
    let at_floor = crate::snapshot::recover_document(&w.conn, b, doc).expect("recover");
    assert_eq!(at_floor.content, reversed(&typed[..5]));
    for step in &result.history.done {
        w.undo_to(b, doc, step.seq);
        crate::snapshot::recover_document(&w.conn, b, doc).expect("every kept position recovers");
    }
}

#[test]
fn an_undo_position_stranded_below_every_snapshot_is_repaired_to_the_newest_text_on_load() {
    let mut w = World::new("one");
    let a = w.session();
    let doc = w.load(a, &alive).doc_id;
    let first = w.type_at_start(a, doc, "x");
    let second = w.type_at_start(a, doc, "y");
    w.snapshot(a, doc, "yxone", second);
    w.conn
        .execute("DELETE FROM snapshots WHERE seq < ?1", params![second])
        .expect("lose the older anchors");
    w.undo_to(a, doc, first);

    let result = w.load(a, &alive);

    assert_eq!(result.recovered.content, "yxone");
    assert_eq!(
        result.history_losses,
        vec![crate::history::HistoryLoss::PositionRepaired]
    );
    assert_eq!(w.position(a, doc), second);
    assert_eq!(w.events_of(a), 2, "repairing the position deletes nothing");
    assert_eq!(
        w.load(a, &alive).history_losses,
        Vec::new(),
        "the repair is permanent"
    );
}

#[test]
fn an_outside_change_that_cuts_off_a_redo_tail_says_so() {
    let mut w = World::new("one");
    let a = w.session();
    let doc = w.load(a, &alive).doc_id;
    let saved = w.type_at_start(a, doc, "x");
    w.save(a, doc, "xone");
    w.type_at_start(a, doc, "y");
    w.undo_to(a, doc, saved);
    w.external_rewrite("rewritten elsewhere");

    let result = w.load(a, &alive);

    assert_eq!(result.recovered.content, "rewritten elsewhere");
    assert_eq!(
        result.history_losses,
        vec![crate::history::HistoryLoss::RedoTailDropped]
    );
}

#[test]
fn an_outside_change_with_nothing_to_redo_loses_nothing() {
    let mut w = World::new("one");
    let a = w.session();
    let doc = w.load(a, &alive).doc_id;
    w.type_at_start(a, doc, "x");
    w.save(a, doc, "xone");
    w.external_rewrite("rewritten elsewhere");

    let result = w.load(a, &alive);

    assert_eq!(result.recovered.content, "rewritten elsewhere");
    assert_eq!(result.history_losses, Vec::new());
}
