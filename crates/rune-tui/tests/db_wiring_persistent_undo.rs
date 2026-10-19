//! Undo history outlives the tab: a document reopened in the same session
//! gets its lineage back from the store, with every historical position
//! mapped exactly onto the durable journal. Driven through
//! `rune_fuzz::Session`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::Path;
use std::sync::Arc;

use rune_db::{DbEvent, LoadResult, OpOutcome};
use rune_fuzz::Session;
use rune_fuzz::driver::wait_for_db_op;
use rune_tui::db::Db;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::Effects;
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs, VfsTestExt};

use db_wiring_common::{publish, restarted_store_at, store_at, temp_db_dir};

const DOC: &str = "/doc.md";

const fn chord(code: KeyCode, shift: bool, sup: bool) -> KeyInput {
    KeyInput {
        code,
        mods: Mods {
            shift,
            alt: false,
            ctrl: false,
            sup,
        },
    }
}

const END: KeyInput = chord(KeyCode::End, false, false);
const SAVE: KeyInput = chord(KeyCode::Char('s'), false, true);
const UNDO: KeyInput = chord(KeyCode::Char('z'), false, true);
const REDO: KeyInput = chord(KeyCode::Char('z'), true, true);

struct Harness {
    session: Session,
    mem: Arc<Mem>,
    vfs: Arc<dyn Vfs + Send + Sync>,
    db_path: std::path::PathBuf,
}

impl Harness {
    fn open(label: &str, initial: &[u8]) -> Harness {
        let db_path = temp_db_dir(label).join("rune-v1.db");
        let mem = Arc::new(Mem::new());
        publish(mem.as_ref(), Path::new(DOC), initial);
        publish(mem.as_ref(), Path::new("/other.md"), b"other");
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
        let (store, bridge) = store_at(&db_path, Arc::clone(&vfs));
        let session = Session::open_with_db(DOC, Arc::clone(&mem), Db::new(store, bridge, false));
        Harness {
            session,
            mem,
            vfs,
            db_path,
        }
    }

    fn content(&mut self) -> String {
        self.session.snapshot().content
    }

    fn settle(&mut self) {
        assert!(self.session.deliver_db_all().is_none());
    }

    fn type_at_end(&mut self, text: &str) {
        assert!(self.session.key(END).is_none());
        assert!(self.session.type_(text).is_none());
        self.settle();
    }

    fn save(&mut self) {
        assert!(self.session.key(SAVE).is_none());
        self.settle();
        assert!(self.session.deliver().is_none());
        self.settle();
    }

    fn undo(&mut self) {
        assert!(self.session.key(UNDO).is_none());
        self.settle();
    }

    fn redo(&mut self) {
        assert!(self.session.key(REDO).is_none());
        self.settle();
    }

    fn undo_all(&mut self) {
        while self.session.app().active_doc().journal.pos() > 0 {
            self.undo();
        }
    }

    fn redo_all(&mut self) {
        let journal_len = |h: &Harness| h.session.app().active_doc().journal.len();
        while self.session.app().active_doc().journal.pos() < journal_len(self) {
            self.redo();
        }
    }

    fn newest_message(&self) -> Option<String> {
        rune_tui::messages::newest_text(self.session.app()).map(str::to_owned)
    }

    fn close_and_reopen(&mut self) {
        let doc_tab = self.session.app().active;
        workspace::open_path(self.session.app_mut(), Path::new("/other.md")).expect("open other");
        self.settle();
        let _ = workspace::close_now(self.session.app_mut(), doc_tab, &mut Effects::default());
        workspace::open_path(self.session.app_mut(), Path::new(DOC)).expect("reopen doc");
        self.settle();
    }

    fn load_through_live_store(&self) -> LoadResult {
        let db = self.session.app().db.as_ref().expect("session has a store");
        let op_id = db.store.load(Path::new(DOC)).expect("enqueue load");
        match wait_for_db_op(&db.bridge, op_id) {
            DbEvent::Ok {
                result: OpOutcome::Load(r),
                ..
            } => *r,
            other => panic!("expected a Load ack, got {other:?}"),
        }
    }

    fn crash_and_restart(self) -> LoadResult {
        let Harness {
            mut session,
            vfs,
            db_path,
            ..
        } = self;
        session
            .app_mut()
            .db
            .take()
            .expect("session has a store")
            .shutdown();
        drop(session);
        let (store, bridge) = restarted_store_at(&db_path, vfs);
        let op_id = store.load(Path::new(DOC)).expect("enqueue load");
        let load = match wait_for_db_op(&bridge, op_id) {
            DbEvent::Ok {
                result: OpOutcome::Load(r),
                ..
            } => *r,
            other => panic!("expected a Load ack, got {other:?}"),
        };
        store.shutdown();
        load
    }
}

/// The reported bug: edit, save, close, open and close another file,
/// reopen, undo.
#[test]
fn undo_after_close_and_reopen_returns_the_pre_edit_text() {
    let mut h = Harness::open("persistent-undo-reported", b"hello");
    h.type_at_end(" world");
    h.save();
    h.type_at_end("!");
    h.save();
    assert_eq!(h.content(), "hello world!");

    h.close_and_reopen();
    assert_eq!(h.content(), "hello world!");

    h.undo_all();
    assert_eq!(h.content(), "hello");
    h.undo();
    assert_eq!(h.newest_message().as_deref(), Some("nothing to undo"));

    h.redo_all();
    assert_eq!(h.content(), "hello world!");
}

#[test]
fn redo_tail_survives_close_and_reopen() {
    let mut h = Harness::open("persistent-undo-redo-tail", b"hello");
    h.type_at_end(" world");
    h.save();
    h.type_at_end("!");
    h.save();
    h.undo();
    assert_eq!(h.content(), "hello world");
    h.save();

    h.close_and_reopen();
    assert_eq!(h.content(), "hello world");

    h.redo();
    assert_eq!(h.content(), "hello world!");
    h.undo_all();
    assert_eq!(h.content(), "hello");
}

#[test]
fn history_of_only_undone_steps_cannot_undo_but_redoes() {
    let mut h = Harness::open("persistent-undo-only-undone", b"hello");
    h.type_at_end(" world");
    h.save();
    h.undo_all();
    h.save();
    assert_eq!(h.mem.read(Path::new(DOC)).expect("file"), b"hello");

    h.close_and_reopen();
    h.undo();
    assert_eq!(h.newest_message().as_deref(), Some("nothing to undo"));
    assert_eq!(h.content(), "hello");

    h.redo_all();
    assert_eq!(h.content(), "hello world");
}

#[test]
fn a_file_never_edited_has_nothing_to_undo_even_after_reopen() {
    let mut h = Harness::open("persistent-undo-fresh", b"hello");
    h.undo();
    assert_eq!(h.newest_message().as_deref(), Some("nothing to undo"));

    h.close_and_reopen();
    h.undo();
    assert_eq!(h.newest_message().as_deref(), Some("nothing to undo"));
    assert_eq!(h.content(), "hello");
}

/// An outside change between close and reopen shows the disk content; the
/// load's replace-all bridge is the newest undoable step, so the first undo
/// returns the user's last saved text.
#[test]
fn outside_change_while_closed_is_one_undo_step_back_to_the_last_saved_text() {
    let mut h = Harness::open("persistent-undo-outside", b"hello");
    h.type_at_end(" world");
    h.save();
    let doc_tab = h.session.app().active;
    workspace::open_path(h.session.app_mut(), Path::new("/other.md")).expect("open other");
    h.settle();
    let _ = workspace::close_now(h.session.app_mut(), doc_tab, &mut Effects::default());
    h.mem
        .save_atomic(Path::new(DOC), b"changed outside")
        .expect("outside change");
    workspace::open_path(h.session.app_mut(), Path::new(DOC)).expect("reopen doc");
    h.settle();
    assert_eq!(h.content(), "changed outside");
    assert!(!h.session.app().active_doc().is_dirty());

    h.undo();
    assert_eq!(h.content(), "hello world");
    h.undo_all();
    assert_eq!(h.content(), "hello");
}

/// Undoing into restored history must resolve to the exact durable position
/// (never a forward replace-all re-base), so the store keeps the whole redo
/// tail and a crash recovers precisely the buffer.
#[test]
fn undo_into_restored_history_keeps_the_durable_journal_exact_across_a_crash() {
    let mut h = Harness::open("persistent-undo-crash", b"hello");
    h.type_at_end(" world");
    h.save();
    h.close_and_reopen();
    let steps = h.session.app().active_doc().journal.len();
    assert!(steps > 0);

    h.undo_all();
    assert_eq!(h.content(), "hello");

    let live = h.load_through_live_store();
    assert_eq!(live.recovered.content, "hello");
    assert_eq!(live.history.done.len(), 0);
    assert_eq!(
        live.history.undone.len(),
        steps,
        "a re-base would have appended a replace-all and cut the redo tail"
    );

    let recovered = h.crash_and_restart();
    assert_eq!(recovered.recovered.content, "hello");
}

/// Typing before the load lands means the tab's journal already diverged
/// from the stored lineage, so history is not installed; the user is told
/// rather than left to find an empty undo.
#[test]
fn typing_while_the_reopen_loads_says_earlier_history_is_unavailable() {
    let mut h = Harness::open("persistent-undo-typed-while-loading", b"hello");
    h.type_at_end(" world");
    h.save();
    let doc_tab = h.session.app().active;
    workspace::open_path(h.session.app_mut(), Path::new("/other.md")).expect("open other");
    h.settle();
    let _ = workspace::close_now(h.session.app_mut(), doc_tab, &mut Effects::default());

    workspace::open_path(h.session.app_mut(), Path::new(DOC)).expect("reopen doc");
    assert!(h.session.type_("!").is_none());
    h.settle();

    assert!(
        rune_tui::messages::log_text(h.session.app())
            .contains("earlier undo history unavailable \u{2014} edited while loading")
    );
    h.undo_all();
    assert_eq!(h.session.app().active_doc().journal.pos(), 0);
}

#[test]
fn a_loaded_history_raises_no_unavailable_notice() {
    let mut h = Harness::open("persistent-undo-no-notice", b"hello");
    h.type_at_end(" world");
    h.save();
    h.close_and_reopen();

    let log = rune_tui::messages::log_text(h.session.app());
    assert!(!log.contains("undo history"), "unexpected notice: {log}");
}
