//! The `rune_fuzz::Session` harness shared by the persistent-undo suites:
//! a real `Store` beside a session, with the keystrokes that drive undo,
//! redo, save, and a tab's close and reopen.

use std::path::Path;
use std::sync::Arc;

use rune_db::{DbEvent, LoadResult, OpOutcome};
use rune_fuzz::Session;
use rune_fuzz::driver::wait_for_db_op;
use rune_tui::db::Db;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::Effects;
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use super::{publish, restarted_store_at, store_at, temp_db_dir};

pub const DOC: &str = "/doc.md";

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

pub struct Harness {
    pub session: Session,
    pub mem: Arc<Mem>,
    pub vfs: Arc<dyn Vfs + Send + Sync>,
    pub db_path: std::path::PathBuf,
}

impl Harness {
    pub fn open(label: &str, initial: &[u8]) -> Harness {
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

    pub fn content(&mut self) -> String {
        self.session.snapshot().content
    }

    pub fn settle(&mut self) {
        assert!(self.session.deliver_db_all().is_none());
    }

    pub fn type_at_end(&mut self, text: &str) {
        assert!(self.session.key(END).is_none());
        assert!(self.session.type_(text).is_none());
        self.settle();
    }

    pub fn save(&mut self) {
        assert!(self.session.key(SAVE).is_none());
        self.settle();
        assert!(self.session.deliver().is_none());
        self.settle();
    }

    pub fn undo(&mut self) {
        assert!(self.session.key(UNDO).is_none());
        self.settle();
    }

    pub fn redo(&mut self) {
        assert!(self.session.key(REDO).is_none());
        self.settle();
    }

    pub fn undo_all(&mut self) {
        while self.session.app().active_doc().journal.pos() > 0 {
            self.undo();
        }
    }

    pub fn redo_all(&mut self) {
        let journal_len = |h: &Harness| h.session.app().active_doc().journal.len();
        while self.session.app().active_doc().journal.pos() < journal_len(self) {
            self.redo();
        }
    }

    pub fn newest_message(&self) -> Option<String> {
        rune_tui::messages::newest_text(self.session.app()).map(str::to_owned)
    }

    pub fn close_and_reopen(&mut self) {
        let doc_tab = self.session.app().active;
        workspace::open_path(self.session.app_mut(), Path::new("/other.md")).expect("open other");
        self.settle();
        let _ = workspace::close_now(self.session.app_mut(), doc_tab, &mut Effects::default());
        workspace::open_path(self.session.app_mut(), Path::new(DOC)).expect("reopen doc");
        self.settle();
    }

    pub fn load_through_live_store(&self) -> LoadResult {
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

    pub fn crash_and_restart(self) -> LoadResult {
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
