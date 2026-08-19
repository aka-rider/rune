//! The quit-confirm state machine (`handle_quit_key`) and its
//! `unpreserved_dirty_docs` guard predicate — `pane_command::
//! handle_global_command`'s `QuitChord` arm is its only production caller
//! now that quit chords resolve at the global pipeline stage; split out of
//! `pane.rs` to keep it under the 500-line budget.

use std::time::Duration;

use crate::app::App;
use crate::document::{Document, DocumentId};
use crate::guard::{self, GuardKind, GuardPrompt};
use crate::keymap::QuitKey;
use crate::runtime::{Effects, Msg};

/// The quit-confirm arm-to-quit window: the first press arms `App::timers`'
/// `TimerKey::QuitConfirm` deadline, carrying the confirm generation.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// The quit-confirm state machine: the SAME
/// chord pressed twice quits; pressing a quit chord while a DIFFERENT one is
/// pending re-arms with the new chord and a fresh generation, restarting the
/// 2s window. `pub(crate)` — moved out of `app.rs` (500-line
/// budget); `handle_global_command` above is its only caller now that quit
/// chords resolve at the global pipeline stage.
pub(crate) fn handle_quit_key(app: &mut App, key: QuitKey, effects: &mut Effects) {
    // Quit is an implicit Esc for an active OR
    // pending merge — exited/cancelled BEFORE the dirty-guard scan below,
    // so that scan (and the guard prompt it may raise) sees the reverted
    // title/plain dirty text, never a stale "editor <-> disk" name for a
    // merge quit is about to end anyway. `auto_exit` (review fix F3)
    // cancels a `Pending` attempt WITH feedback instead of silently
    // discarding it.
    if !matches!(app.merge, crate::merge::MergeState::Inactive) {
        crate::merge::auto_exit(app);
    }
    // Quit is a destructive transition on every dirty document at once, and
    // the 2-press confirm above is only a safe shortcut BECAUSE quit
    // preserves through the durable journal. That premise
    // fails for any dirty document with no live `db` binding (the default
    // untitled draft by construction, or an Explorer/CLI-opened document
    // whose hydration never landed) — for those, quitting would discard
    // work with no journal to recover it from. Raise a Guard instead of
    // arming or completing quit. It carries `DirtyQuit`, not `DirtyClose`:
    // the answer must finish the quit the user asked for (discard exits;
    // save exits once every started save acks), because a Guard whose
    // answers only ever CLOSED left a single-document session with no
    // reachable exit at all.
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

    if let crate::app::QuitNegotiation::ConfirmArmed(pending_key, generation) = app.quit
        && pending_key == key
    {
        let _ = generation; // the SAME chord always quits regardless of generation
        app.should_quit = true;
        return;
    }

    let generation = app.next_quit_gen.mint();
    app.quit = crate::app::QuitNegotiation::ConfirmArmed(key, generation);
    app.timers.arm(
        crate::runtime::TimerKey::QuitConfirm,
        CONFIRM_TIMEOUT,
        Msg::ConfirmTimeout { generation },
    );
}

/// Every open document that is both dirty and has no live, trustworthy
/// recovery-store binding (`App::is_preserved`) — quit preserves through the
/// durable journal, so a dirty document without one is the exact case
/// `handle_quit_key`'s Guard gate exists for, and the exact set the
/// quit-save fan-out (`guard`'s `[S]ave` answer) must save every
/// member of, not just the first. Deterministic ordering (`documents` is a
/// `BTreeMap`) rather than "whichever `HashMap` bucket happens to iterate
/// first" — repeated presses always raise the Guard for the same document
/// until it's resolved.
/// `handle_quit_key`'s own Guard-raise takes just the first (lowest-id) one;
/// the quit-save fan-out iterates the whole `Vec`.
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

    /// A live, non-degraded app-level `Db` (mirrors `db_ack.rs::tests::
    /// in_memory_db`) — `App::is_preserved` requires one to exist (not just
    /// a document's own `DocDb`) before it will call a document preserved,
    /// since `degraded` lives on the app-level handle.
    fn live_db() -> Db {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let clock: ClockFn = Arc::new(std::time::SystemTime::now);
        let store = Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
        let bridge = DbBridge::bootstrap();
        Db::new(store, bridge, false)
    }

    /// A dirty document with no live
    /// `db` binding (the default for an untitled draft) must never be
    /// silently discarded by the quit chord — `^C^C` (or `^D^D`) raises a
    /// `DirtyQuit` Guard rather than quitting or merely
    /// closing.
    #[test]
    fn double_quit_chord_on_an_unpreserved_dirty_doc_raises_a_guard_instead_of_quitting() {
        let mut app = app();
        // `is_dirty` compares both the live version and the live bytes
        // against the saved baseline, so a genuinely dirty fixture needs a
        // real edit, exactly like a keystroke would make.
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

    /// The converse: a dirty document that IS preserved (has a live `db`
    /// binding) doesn't trip the new gate — the ordinary two-press
    /// quit-confirm still works exactly as before.
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

    /// The quit chord, resolved through `App::update`'s real `Msg::Key`
    /// dispatch, never reaches `handle_quit_key` at all while a Guard is
    /// already showing (`dispatch::handle_key`'s Stage 1 routes every key
    /// to the existing prompt first) — so a foreign Guard already up is
    /// exercised by calling `handle_quit_key` directly, exactly the real
    /// entry point `Command::QuitConfirm` resolves to; the two paths only
    /// ever differ by that already-showing-prompt short circuit, never by
    /// what `handle_quit_key` itself does. A DirtyQuit raise attempt against
    /// an occupied slot must warn and leave the original prompt alone,
    /// rather than silently dropping the quit intent.
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
