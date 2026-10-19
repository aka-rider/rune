//! Undo history outlives the process: a document reopened by a brand-new
//! session (the previous one dead) steps back through the dead session's
//! edits one at a time. Driven through `rune_fuzz::Session`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod db_wiring_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_fuzz::Session;
use rune_fuzz::action::Action;
use rune_tui::db::Db;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_vfs::{Mem, Vfs, VfsTestExt};

use db_wiring_common::{publish, restarted_store_at, store_at, temp_db_dir};

const DOC: &str = "/doc.md";

const fn chord(code: KeyCode, shift: bool) -> KeyInput {
    KeyInput {
        code,
        mods: Mods {
            shift,
            alt: false,
            ctrl: false,
            sup: true,
        },
    }
}

const END: KeyInput = KeyInput {
    code: KeyCode::End,
    mods: Mods::NONE,
};
const SAVE: KeyInput = chord(KeyCode::Char('s'), false);
const UNDO: KeyInput = chord(KeyCode::Char('z'), false);
const REDO: KeyInput = chord(KeyCode::Char('z'), true);

struct Run {
    mem: Arc<Mem>,
    vfs: Arc<dyn Vfs + Send + Sync>,
    db_path: PathBuf,
    session: Session,
}

impl Run {
    fn first_launch(label: &str, initial: &[u8]) -> Run {
        let db_path = temp_db_dir(label).join("rune-v1.db");
        let mem = Arc::new(Mem::new());
        publish(mem.as_ref(), Path::new(DOC), initial);
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
        let (store, bridge) = store_at(&db_path, Arc::clone(&vfs));
        let session = Session::open_with_db(DOC, Arc::clone(&mem), Db::new(store, bridge, false));
        Run {
            mem,
            vfs,
            db_path,
            session,
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

    fn press_after_ladder_reset(&mut self, key: KeyInput) {
        let millis = u64::try_from(rune_tui::undogroup::LADDER_RESET.as_millis()).expect("fits");
        assert!(self.session.act(Action::AdvanceClock(millis)).is_none());
        assert!(self.session.key(key).is_none());
        self.settle();
    }

    fn undo(&mut self) {
        self.press_after_ladder_reset(UNDO);
    }

    fn redo(&mut self) {
        self.press_after_ladder_reset(REDO);
    }

    fn assert_nothing_to_undo(&mut self) {
        self.undo();
        assert_eq!(
            rune_tui::messages::newest_text(self.session.app()),
            Some("nothing to undo")
        );
    }

    fn assert_nothing_to_redo(&mut self) {
        self.redo();
        assert_eq!(
            rune_tui::messages::newest_text(self.session.app()),
            Some("nothing to redo")
        );
    }

    /// The process ends: the store drains its writer, the session is gone,
    /// and a brand-new session opens the same document on the same
    /// database with every earlier session reported dead.
    fn restart(self) -> Run {
        let Run {
            mem,
            vfs,
            db_path,
            mut session,
        } = self;
        session
            .app_mut()
            .db
            .take()
            .expect("session has a store")
            .shutdown();
        drop(session);
        let (store, bridge) = restarted_store_at(&db_path, Arc::clone(&vfs));
        let mut session =
            Session::open_with_db(DOC, Arc::clone(&mem), Db::new(store, bridge, false));
        assert!(session.deliver_db_all().is_none());
        Run {
            mem,
            vfs,
            db_path,
            session,
        }
    }
}

#[test]
fn saved_edits_undo_step_by_step_after_a_restart_and_redo_back() {
    let mut run = Run::first_launch("restart-undo-saved", b"base");
    for part in ["1", "2", "3"] {
        run.type_at_end(part);
        run.save();
    }
    assert_eq!(run.content(), "base123");

    let mut run = run.restart();
    assert_eq!(run.content(), "base123");

    for expected in ["base12", "base1", "base"] {
        run.undo();
        assert_eq!(run.content(), expected);
    }
    run.assert_nothing_to_undo();

    for expected in ["base1", "base12", "base123"] {
        run.redo();
        assert_eq!(run.content(), expected);
    }
    run.assert_nothing_to_redo();
}

#[test]
fn unsaved_edits_survive_a_crash_and_undo_step_back_to_the_disk_text() {
    let mut run = Run::first_launch("restart-undo-crash", b"base");
    for part in ["1", "2", "3"] {
        run.type_at_end(part);
    }
    assert_eq!(
        run.mem.read(Path::new(DOC)).expect("file"),
        b"base",
        "the crashed session never saved"
    );

    let mut run = run.restart();
    assert_eq!(run.content(), "base123");
    assert!(run.session.app().active_doc().is_dirty());

    for expected in ["base12", "base1", "base"] {
        run.undo();
        assert_eq!(run.content(), expected);
    }
    run.assert_nothing_to_undo();
}

#[test]
fn an_outside_change_between_runs_shows_the_disk_text_and_undoes_to_the_last_saved_text() {
    let mut run = Run::first_launch("restart-undo-outside", b"base");
    for part in ["1", "2"] {
        run.type_at_end(part);
        run.save();
    }

    run.mem
        .save_atomic(Path::new(DOC), b"changed outside")
        .expect("outside change");
    let mut run = run.restart();
    assert_eq!(run.content(), "changed outside");
    assert!(!run.session.app().active_doc().is_dirty());

    run.undo();
    assert_eq!(run.content(), "base12");
    run.undo();
    assert_eq!(run.content(), "base1");
    run.undo();
    assert_eq!(run.content(), "base");
    run.assert_nothing_to_undo();
}
