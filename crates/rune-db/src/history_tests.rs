#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::SystemTime;

use rusqlite::{Connection, params};

use rune_core::buffer::AppliedEdit;
use rune_core::coords::{BufferOffset, VisualCol};
use rune_core::cursor::{Cursor, CursorId};
use rune_core::undo::EditKind;

use super::*;
use crate::journal::{append_edit, move_undo_pos};
use crate::journal_append::EditBatch;
use crate::snapshot::{create_snapshot, recover_document};
use crate::test_support::{insert_test_document, open};

struct Fixture {
    conn: Connection,
    session: SessionId,
    doc: DocId,
}

fn fixture(base: &str) -> Fixture {
    let mut conn = open();
    let session = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
    let tx = conn.transaction().expect("tx");
    let doc = insert_test_document(&tx);
    create_snapshot(&tx, session, SystemTime::now(), doc, base, Seq(0)).expect("base snapshot");
    tx.commit().expect("commit");
    Fixture { conn, session, doc }
}

fn cursor_at(position: usize) -> Cursor {
    Cursor {
        position: BufferOffset(position),
        anchor: BufferOffset(position),
        desired_col: VisualCol(0),
        id: CursorId::FIRST,
    }
}

fn insert(start: usize, text: &str) -> AppliedEdit {
    AppliedEdit {
        start,
        end: start,
        deleted: String::new(),
        insert: text.to_string(),
    }
}

impl Fixture {
    fn append(&mut self, edit: AppliedEdit, before: &[Cursor], after: &[Cursor]) -> Seq {
        let tx = self.conn.transaction().expect("tx");
        let seq = append_edit(
            &tx,
            self.session,
            SystemTime::now(),
            self.doc,
            EditBatch {
                edits: &[edit],
                cursors_before: before,
                cursors_after: after,
                kind: EditKind::Insert,
            },
        )
        .expect("append");
        tx.commit().expect("commit");
        seq
    }

    fn type_text(&mut self, from_len: usize, text: &str) -> Vec<Seq> {
        text.chars()
            .enumerate()
            .map(|(i, ch)| self.append(insert(from_len + i, &ch.to_string()), &[], &[]))
            .collect()
    }

    fn undo_to(&mut self, seq: Seq) {
        let tx = self.conn.transaction().expect("tx");
        move_undo_pos(&tx, self.session, self.doc, seq).expect("move");
        tx.commit().expect("commit");
    }

    fn read(&mut self, shown: &str) -> (Lineage, Option<HistoryLoss>) {
        let tx = self.conn.transaction().expect("tx");
        let read = read_lineage(&tx, self.session, self.doc, shown).expect("read_lineage");
        tx.commit().expect("commit");
        read
    }

    fn recovered_at(&mut self, seq: Seq) -> String {
        self.undo_to(seq);
        recover_document(&self.conn, self.session, self.doc)
            .expect("recover")
            .content
    }

    fn current_seq(&mut self) -> Seq {
        let tx = self.conn.transaction().expect("tx");
        crate::journal::current_seq(&tx, self.session, self.doc).expect("current_seq")
    }

    fn event_count(&self) -> i64 {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE doc_id=?1 AND session_id=?2",
                params![self.doc, self.session],
                |r| r.get(0),
            )
            .expect("count")
    }
}

#[test]
fn a_document_without_edits_has_an_empty_history_at_its_snapshot() {
    let mut f = fixture("hello");

    let (lineage, loss) = f.read("hello");

    assert_eq!(lineage, Lineage::default());
    assert_eq!(loss, None);
    assert_eq!(lineage.position_seq(), Seq(0));
}

#[test]
fn a_snapshot_taken_mid_history_does_not_shorten_the_lineage() {
    let mut f = fixture("hi");
    let seqs = f.type_text(2, "abc");
    let tx = f.conn.transaction().expect("tx");
    create_snapshot(&tx, f.session, SystemTime::now(), f.doc, "hiab", seqs[1]).expect("autosave");
    tx.commit().expect("commit");

    let (lineage, loss) = f.read("hiabc");

    assert_eq!(loss, None);
    assert_eq!(lineage.base_seq, Seq(0));
    assert_eq!(lineage.done.len(), 3);
}

#[test]
fn a_history_of_only_undone_steps_has_nothing_done_and_everything_to_redo() {
    let mut f = fixture("hi");
    let seqs = f.type_text(2, "abc");
    f.undo_to(Seq(0));

    let (lineage, loss) = f.read("hi");

    assert_eq!(loss, None);
    assert!(lineage.done.is_empty());
    assert_eq!(lineage.undone.len(), 3);
    assert_eq!(lineage.position_seq(), Seq(0));
    assert_eq!(lineage.seqs().collect::<Vec<_>>(), seqs);
}

#[test]
fn the_undo_position_splits_the_history_into_done_and_undone_steps() {
    let mut f = fixture("hi");
    let seqs = f.type_text(2, "abc");
    f.undo_to(seqs[1]);

    let (lineage, loss) = f.read("hiab");

    assert_eq!(loss, None);
    assert_eq!(
        lineage.done.iter().map(|s| s.seq).collect::<Vec<_>>(),
        seqs[..2]
    );
    assert_eq!(
        lineage.undone.iter().map(|s| s.seq).collect::<Vec<_>>(),
        seqs[2..]
    );
    assert_eq!(lineage.position_seq(), seqs[1]);
    assert_eq!(lineage.done[0].step.kind, EditKind::Insert);
    assert_eq!(lineage.done[0].step.edits, vec![insert(2, "a")]);
}

#[test]
fn a_step_that_deletes_text_the_buffer_does_not_hold_makes_the_history_unreplayable() {
    let mut f = fixture("hello");
    f.append(insert(5, "!"), &[], &[]);
    let head = f.append(
        AppliedEdit {
            start: 0,
            end: 1,
            deleted: "X".to_string(),
            insert: String::new(),
        },
        &[],
        &[],
    );

    let (lineage, loss) = f.read("ello!");

    assert_eq!(loss, Some(HistoryLoss::Unreplayable));
    assert_eq!(
        lineage,
        Lineage {
            base_seq: head,
            ..Lineage::default()
        },
        "an unreplayable history is an empty one at the current position"
    );
    assert_eq!(f.event_count(), 2, "reading never deletes a journal row");
    assert_eq!(f.current_seq(), head);
    assert_eq!(
        recover_document(&f.conn, f.session, f.doc)
            .expect("recover")
            .content,
        "ello!",
        "the shown content stays recoverable"
    );
}

#[test]
fn a_history_that_replays_to_other_content_than_shown_is_unreplayable() {
    let mut f = fixture("hello");
    f.append(insert(5, "!"), &[], &[]);

    let (_, loss) = f.read("hello?");

    assert_eq!(loss, Some(HistoryLoss::Unreplayable));
}

#[test]
fn an_unreplayable_history_keeps_its_redo_tail_on_disk() {
    let mut f = fixture("ab");
    let seqs = f.type_text(2, "cd");
    f.undo_to(seqs[0]);

    let (lineage, loss) = f.read("abXX");

    assert_eq!(loss, Some(HistoryLoss::Unreplayable));
    assert_eq!(lineage.base_seq, seqs[0]);
    assert_eq!(f.current_seq(), seqs[0]);
    assert_eq!(f.event_count(), 2, "the redo tail is the user's words");
    assert_eq!(f.recovered_at(seqs[1]), "abcd");
}

#[test]
fn cursors_inside_a_character_or_past_the_end_are_clamped_to_the_buffer() {
    let mut f = fixture("é");
    f.append(insert(2, "x"), &[cursor_at(1)], &[cursor_at(99)]);

    let (lineage, loss) = f.read("éx");

    assert_eq!(loss, None);
    let step = &lineage.done[0].step;
    assert_eq!(step.cursors_before, vec![cursor_at(0)]);
    assert_eq!(step.cursors_after, vec![cursor_at(3)]);
}

#[test]
fn a_history_past_the_cap_shows_its_newest_steps_and_deletes_nothing() {
    let text: String = ('a'..='z').cycle().take(1005).collect();
    let mut f = fixture("");
    let seqs = f.type_text(0, &text);

    let (lineage, loss) = f.read(&text);

    assert_eq!(loss, None);
    assert_eq!(lineage.done.len(), 1000);
    assert_eq!(lineage.base_seq, seqs[4]);
    assert_eq!(f.event_count(), 1005, "reading is a pure read");
    assert_eq!(f.read(&text), (lineage.clone(), None));
    assert_eq!(f.recovered_at(lineage.base_seq), text[..5]);
    for (kept, step) in lineage.done.iter().enumerate() {
        assert_eq!(f.recovered_at(step.seq), text[..5 + kept + 1]);
    }
}

#[test]
fn the_cap_never_drops_a_step_at_or_after_the_undo_position() {
    let text: String = ('a'..='z').cycle().take(1005).collect();
    let mut f = fixture("");
    let seqs = f.type_text(0, &text);
    f.undo_to(seqs[2]);

    let (lineage, loss) = f.read(&text[..3]);

    assert_eq!(loss, None);
    assert_eq!(lineage.base_seq, seqs[2]);
    assert!(lineage.done.is_empty());
    assert_eq!(lineage.undone.len(), 1002);
}

#[test]
fn a_capped_history_replays_from_the_snapshot_nearest_its_floor() {
    let text: String = ('a'..='z').cycle().take(1005).collect();
    let mut f = fixture("");
    let seqs = f.type_text(0, &text);
    let tx = f.conn.transaction().expect("tx");
    create_snapshot(
        &tx,
        f.session,
        SystemTime::now(),
        f.doc,
        &text[..5],
        seqs[4],
    )
    .expect("snap");
    tx.commit().expect("commit");
    f.conn
        .execute(
            "UPDATE events SET edits='{' WHERE seq < ?1",
            params![seqs[4]],
        )
        .expect("corrupt rows below the floor snapshot");

    let (lineage, loss) = f.read(&text);

    assert_eq!(loss, None, "rows below the floor are never decoded");
    assert_eq!(lineage.done.len(), 1000);
}

#[test]
fn a_snapshot_blob_that_is_gone_loses_the_history_but_not_the_content() {
    let mut f = fixture("hi");
    let seqs = f.type_text(2, "abc");
    let tx = f.conn.transaction().expect("tx");
    create_snapshot(&tx, f.session, SystemTime::now(), f.doc, "hiab", seqs[1]).expect("snapshot");
    tx.commit().expect("commit");
    f.conn
        .execute_batch("PRAGMA foreign_keys=OFF")
        .expect("pragma");
    f.conn
        .execute(
            "DELETE FROM blobs WHERE hash=?1",
            params![crate::observation::hash_bytes(b"hi")],
        )
        .expect("lose the oldest snapshot's blob");

    let (lineage, loss) = f.read("hiabc");

    assert_eq!(loss, Some(HistoryLoss::Unreadable));
    assert_eq!(
        lineage,
        Lineage {
            base_seq: seqs[2],
            ..Lineage::default()
        }
    );
    assert_eq!(
        recover_document(&f.conn, f.session, f.doc)
            .expect("recover from the newer snapshot")
            .content,
        "hiabc"
    );
}

#[test]
fn a_corrupt_event_row_below_the_newest_snapshot_loses_the_history_but_not_the_content() {
    let mut f = fixture("hi");
    let seqs = f.type_text(2, "abc");
    let tx = f.conn.transaction().expect("tx");
    create_snapshot(&tx, f.session, SystemTime::now(), f.doc, "hiab", seqs[1]).expect("snapshot");
    tx.commit().expect("commit");
    f.conn
        .execute("UPDATE events SET edits='{' WHERE seq=?1", params![seqs[0]])
        .expect("corrupt an old row");

    let (lineage, loss) = f.read("hiabc");

    assert_eq!(loss, Some(HistoryLoss::Unreadable));
    assert!(lineage.done.is_empty() && lineage.undone.is_empty());
    assert_eq!(
        recover_document(&f.conn, f.session, f.doc)
            .expect("recover")
            .content,
        "hiabc"
    );
}
