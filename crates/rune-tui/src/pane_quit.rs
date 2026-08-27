use std::time::Duration;

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::guard::{self, GuardKind, GuardPrompt};
use crate::keymap::QuitKey;
use crate::runtime::{Effects, Msg};

const CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) fn handle_quit_key(app: &mut App, key: QuitKey, effects: &mut Effects) {
    if !matches!(app.merge, crate::merge::MergeState::Inactive) {
        crate::merge::auto_exit(app);
    }
    if let Some(doc) = unpreserved_dirty_docs(app).into_iter().next() {
        let _ = guard::set_guard_or_warn(
            app,
            GuardPrompt {
                doc,
                kind: GuardKind::DirtyQuit,
            },
            "quit confirmation dropped \u{2014} a prompt is already showing",
            effects,
        );
        return;
    }

    if let crate::app::QuitNegotiation::ConfirmArmed(pending_key, _) = app.quit
        && pending_key == key
    {
        app.should_quit = true;
        return;
    }

    let generation = app.next_quit_gen.mint();
    app.quit = crate::app::QuitNegotiation::ConfirmArmed(key, generation);
    app.timers.arm(
        crate::runtime::TimerKey::from(crate::runtime::TimerMsgKey::QuitConfirm),
        CONFIRM_TIMEOUT,
        Msg::Timer {
            key: crate::runtime::TimerMsgKey::QuitConfirm,
            generation: generation.raw(),
        },
    );
}

pub(crate) fn unpreserved_dirty_docs(app: &mut App) -> Vec<DocumentId> {
    let candidates: Vec<DocumentId> = app.documents.keys().copied().collect();
    candidates
        .into_iter()
        .filter(|&id| {
            let preserved = app.doc(id).is_some_and(|d| app.is_preserved(d));
            !preserved && app.doc(id).is_some_and(Document::is_dirty)
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::db::{Db, DbBridge};
    use crate::document::Replica;
    use rune_core::buffer::Buffer;
    use rune_db::{ClockFn, Store};
    use rune_vfs::{Mem, Vfs};
    use std::sync::Arc;

    fn app() -> App {
        App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
    }

    fn fx() -> Effects {
        Effects::default()
    }

    fn live_db() -> Db {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let clock: ClockFn = Arc::new(std::time::SystemTime::now);
        let store = Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
        let bridge = DbBridge::bootstrap();
        Db::new(store, bridge, false)
    }

    #[test]
    fn double_quit_chord_on_an_unpreserved_dirty_doc_raises_a_guard_instead_of_quitting() {
        let mut app = app();
        let active = app.active;
        crate::commands::edit::insert_char(&mut app, active, '!');
        assert!(
            !app.active_doc().is_store_bound(),
            "test setup: no db binding"
        );

        handle_quit_key(&mut app, QuitKey::CtrlC, &mut fx());
        handle_quit_key(&mut app, QuitKey::CtrlC, &mut fx());

        assert!(
            !app.should_quit,
            "quit must not complete while unpreserved dirty work exists"
        );
        assert!(
            matches!(
                app.guard,
                Some(GuardPrompt {
                    kind: GuardKind::DirtyQuit,
                    ..
                })
            ),
            "expected a DirtyQuit Guard prompt to be raised"
        );
    }

    #[test]
    fn double_quit_chord_on_a_preserved_dirty_doc_still_quits() {
        let mut app = app();
        let active = app.active;
        crate::commands::edit::insert_char(&mut app, active, '!');
        app.doc_mut(app.active).expect("active doc exists").replica = Replica::Bound(
            crate::db::DocDb::new(1, crate::db::PublishMode::CreateOnly, rune_db::Seq(0)),
        );
        app.db = Some(live_db());

        handle_quit_key(&mut app, QuitKey::CtrlC, &mut fx());
        assert!(!app.should_quit, "the first press only arms the confirm");
        handle_quit_key(&mut app, QuitKey::CtrlC, &mut fx());
        assert!(app.should_quit, "the second matching press quits");
    }

    #[test]
    fn quit_chord_while_a_different_guard_is_up_warns_and_preserves_it() {
        let mut app = app();
        let active = app.active;
        crate::commands::edit::insert_char(&mut app, active, '!');
        assert!(
            !app.active_doc().is_store_bound(),
            "test setup: no db binding"
        );
        let other_doc = app.active;
        assert_eq!(
            crate::guard::set_guard(
                &mut app,
                GuardPrompt {
                    doc: other_doc,
                    kind: GuardKind::DiskConflict,
                },
                &mut fx()
            ),
            crate::guard::GuardRaise::Raised,
            "test setup: pre-arm a foreign guard"
        );

        handle_quit_key(&mut app, QuitKey::CtrlC, &mut fx());

        assert!(!app.should_quit);
        assert!(
            matches!(
                app.guard,
                Some(GuardPrompt {
                    kind: GuardKind::DiskConflict,
                    ..
                })
            ),
            "the pre-existing prompt must survive unchanged"
        );
        assert_eq!(
            crate::messages::newest_text(&app),
            Some("quit confirmation dropped \u{2014} a prompt is already showing")
        );
    }
}
