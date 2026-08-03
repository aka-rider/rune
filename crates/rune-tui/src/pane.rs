//! `Pane` — the focus discriminant (plan Context, decision 7: "Pane enum =
//! focus discriminant only" — no trait objects; `Explorer`/`Tabs`'s own
//! state lands in plain named `App` fields in WP4/WP5). Extracted out of
//! `app.rs` to keep it under the §1.6 budget (plan WP2 Rules: "extract to
//! pane.rs ... as needed") — `handle_global_command` lives here too since
//! it's the sole reader/writer of `App::focus`/the left column's `Split`
//! state outside `app.rs` itself.

use std::time::Duration;

use crate::app::App;
use crate::banner::{self, GuardKind, GuardPrompt, Modal};
use crate::document::DocumentId;
use crate::explorer;
use crate::keymap::{GlobalCommand, QuitKey};
use crate::runtime::{Cmd, CmdKind, Effects, Msg};
use crate::save;

/// The quit-confirm arm-to-quit window (plan Context, "Quit-confirm": "first
/// press arms + spawns 2s timer Cmd carrying gen").
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// Which chrome region owns the next keystroke once the global table
/// (`keymap::GLOBAL_BINDINGS`) doesn't claim it (plan Context, decision 8's
/// four-stage key pipeline, stage 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Explorer,
    Tabs,
    Editor,
    /// The editable title field (`title.rs`) — focused by `^r` or by
    /// pressing Up at the top of the editor. While it owns focus every
    /// keystroke goes to the file name and none of them reach the buffer.
    Title,
}

/// Stage 2 of the four-stage key pipeline (plan Context, decision 8,
/// `app::handle_key`): every `GlobalCommand` fires regardless of which pane
/// currently has focus — the quit chords and Save in particular must keep
/// working while the Explorer/Tabs stub panes own it (plan WP2.S4).
pub(crate) fn handle_global_command(app: &mut App, cmd: GlobalCommand, effects: &mut Effects) {
    // ONE hoisted gate, deliberately before the match (plan "Keybinding"):
    // a global chord pressed while the title is focused commits the typed
    // name FIRST, so ⌘S can never save under the old name and the edit is
    // never silently discarded. A no-op when the title isn't focused. NEVER
    // an early return: a refused commit leaves focus on the title with the
    // reason already in the footer, but every arm below must stay reachable
    // regardless — quit, save and close would otherwise be unreachable for a
    // user holding an unusable name. Decision 7 is what keeps a repeated,
    // idempotent blur (each arm's own `set_focus` re-entering `on_blur`)
    // harmless.
    app.blur_title(effects);

    match cmd {
        GlobalCommand::FocusExplorer => {
            // Always exposes and focuses the Explorer — never hides it, so
            // the command a user reaches for to SEE the Explorer can never
            // instead take it away (mirrors the Go reference's own
            // show-plus-focus contract, and `FocusTabs`'s below).
            app.splits.left.show();
            app.splits.explorer.show();
            app.set_focus(Pane::Explorer, effects);
            // Shared with the startup path that shows this column before
            // any key is pressed, so both fill the pane identically.
            explorer::ensure_loaded(app, effects);
        }
        GlobalCommand::FocusEditor => app.set_focus(Pane::Editor, effects),
        // Entering the title needs no `Effects` — it can never itself leave
        // it (decision 5). Reseeds from the document that is actually
        // showing, every time: the field must never present a stale name
        // from a previous document or a previously abandoned edit (no
        // shadow state).
        GlobalCommand::FocusTitle => app.focus_title(),
        // Mirrors `FocusExplorer`'s "show + focus" pairing: the Tabs pane's
        // own cursor is meaningless to a user who can't see it. Also makes
        // sure the tab rows themselves have room — a starved split from a
        // dragged-down divider is raised back to its floor before focus
        // lands there. No dir-load side effect needed here — unlike
        // Explorer, Tabs has nothing to lazily fetch off-thread.
        GlobalCommand::FocusTabs => {
            app.splits.left.show();
            let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
            let geo = crate::layout::geometry(area, app);
            if let Some(block) = geo.left_block {
                let budget = crate::layout::explorer_budget(block);
                app.splits
                    .explorer
                    .ensure_trail(budget, crate::layout::TABS_LIMITS);
            }
            app.set_focus(Pane::Tabs, effects);
        }
        GlobalCommand::CollapseLeft => {
            app.splits.left.hide();
            if matches!(app.focus(), Pane::Explorer | Pane::Tabs) {
                app.set_focus(Pane::Editor, effects);
            }
        }
        GlobalCommand::Save => {
            let _ = save::trigger_save(app, app.active, effects);
        }
        // WP7.S2: mints/toggles the generated Help virtual document — a
        // direct, same-tick call (decision 10), no I/O involved. The hoisted
        // gate above already blurred, but that gate fires only for
        // `Pane::Title` — without moving focus here too, `F1` pressed from
        // the Explorer or Tabs pane would switch the active document while
        // focus stayed stranded on the chrome list (WP2.S8).
        GlobalCommand::Help => {
            app.set_focus(Pane::Editor, effects);
            crate::workspace::toggle_help(app);
        }
        GlobalCommand::QuitChord(key) => handle_quit_key(app, key, effects),
        // Routes through the one close chokepoint regardless of which pane
        // held focus when `^w` was pressed, so a dirty document still arms
        // its Guard exactly like the Tabs-pane-local close it replaces.
        GlobalCommand::CloseFile => crate::workspace::request_close(app, app.active, effects),
        // Out-of-range is a silent no-op, so a digit naming a tab that
        // isn't open does nothing rather than guessing at a neighbour. Same
        // pre-switch focus move as `Help` above, and for the same reason.
        GlobalCommand::TabSwitch(idx) => {
            app.set_focus(Pane::Editor, effects);
            crate::workspace::switch_to_index(app, idx);
        }
    }
}

/// Port of the quit-confirm state machine (plan Context, "Quit-confirm",
/// mirroring Go `footer.go`): the SAME chord pressed twice quits;
/// pressing a quit chord while a DIFFERENT one is pending re-arms with the
/// new chord and a fresh generation, restarting the 2s window. `pub(crate)`
/// — WP2 moved this out of `app.rs` (§1.6 budget); `handle_global_command`
/// above is its only caller now that quit chords resolve at the global
/// pipeline stage.
pub(crate) fn handle_quit_key(app: &mut App, key: QuitKey, effects: &mut Effects) {
    // §1.4.4: quit is a destructive transition on every dirty document at
    // once, and the 2-press confirm above is only a safe shortcut BECAUSE
    // §12 assumes quit preserves through the durable journal. That premise
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
        let _ = banner::set_modal(
            app,
            Modal::Guard(GuardPrompt {
                doc,
                kind: GuardKind::DirtyQuit,
            }),
        );
        return;
    }

    if let Some((pending_key, generation)) = app.pending_quit
        && pending_key == key
    {
        let _ = generation; // the SAME chord always quits regardless of generation
        app.should_quit = true;
        return;
    }

    let generation = app.next_quit_gen;
    app.next_quit_gen = app.next_quit_gen.wrapping_add(1);
    app.pending_quit = Some((key, generation));
    effects.cmds.push(quit_confirm_timeout_cmd(generation));
}

/// Every open document that is both dirty and has no live, trustworthy
/// recovery-store binding (`App::is_preserved`) — quit preserves through the
/// durable journal, so a dirty document without one is the exact case
/// `handle_quit_key`'s Guard gate exists for, and the exact set WP2's
/// quit-save fan-out (`banner::guard`'s `[S]ave` answer) must save every
/// member of, not just the first. Deterministic ordering (`documents` is a
/// `BTreeMap`) rather than "whichever `HashMap` bucket happens to iterate
/// first" — repeated presses always raise the Guard for the same document
/// until it's resolved. Dirty is re-derived via `is_dirty_now`, not read
/// from the cache (CONSTITUTION §1.4.8: quit is a transition), so a stale
/// cache can never wave a genuinely-dirty document through the guard.
/// `handle_quit_key`'s own Guard-raise takes just the first (lowest-id) one;
/// the quit-save fan-out iterates the whole `Vec`.
pub(crate) fn unpreserved_dirty_docs(app: &mut App) -> Vec<DocumentId> {
    let candidates: Vec<DocumentId> = app.documents.keys().copied().collect();
    candidates
        .into_iter()
        .filter(|&id| {
            let preserved = app.doc(id).is_some_and(|d| app.is_preserved(d));
            !preserved && crate::materialize_ack::is_dirty_now(app, id)
        })
        .collect()
}

/// The 2s quit-confirm timer, carrying its generation so a stale timeout
/// (superseded by a second press or a re-arm) is ignored on arrival.
/// Genuine wall-clock pacing for a real UI feature — not a test-ordering
/// hack — so `std::thread::sleep` here is correct (this `Cmd` runs on its
/// own dedicated thread by runtime design, never blocking the main loop).
fn quit_confirm_timeout_cmd(generation: u32) -> Cmd {
    Cmd::new(CmdKind::QuitTimeout, move || {
        std::thread::sleep(CONFIRM_TIMEOUT);
        Some(Msg::ConfirmTimeout { generation })
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::db::{Db, DbBridge};
    use rune_core::buffer::Buffer;
    use rune_db::{ClockFn, Store};
    use rune_vfs::{Mem, Vfs};
    use std::sync::Arc;

    fn app() -> App {
        App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
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

    #[test]
    fn focus_explorer_shows_the_left_pane_and_focuses_it() {
        let mut app = app();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::FocusExplorer, &mut effects);
        assert!(app.splits.left.is_shown());
        assert_eq!(app.focus(), Pane::Explorer);
    }

    /// The command that shows the Explorer must never be the one that hides
    /// it again — pressing it a second time is a no-op on visibility, not a
    /// toggle back off.
    #[test]
    fn pressing_it_twice_keeps_the_explorer_shown_and_focused() {
        let mut app = app();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::FocusExplorer, &mut effects);
        handle_global_command(&mut app, GlobalCommand::FocusExplorer, &mut effects);
        assert!(app.splits.left.is_shown());
        assert_eq!(app.focus(), Pane::Explorer);
    }

    /// The collapse command hides the column and, only when it currently
    /// owns focus, hands focus back to the Editor rather than leaving a
    /// keystroke routed to a pane with no on-screen presence.
    #[test]
    fn collapse_left_hides_the_column_and_returns_focus_to_the_editor() {
        let mut app = app();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::FocusExplorer, &mut effects);
        handle_global_command(&mut app, GlobalCommand::CollapseLeft, &mut effects);
        assert!(!app.splits.left.is_shown());
        assert_eq!(app.focus(), Pane::Editor);
    }

    #[test]
    fn focus_editor_returns_focus_regardless_of_the_left_columns_visibility() {
        let mut app = app();
        let mut effects = Effects::default();
        app.set_focus(Pane::Explorer, &mut effects);
        app.splits.left.show();
        handle_global_command(&mut app, GlobalCommand::FocusEditor, &mut effects);
        assert_eq!(app.focus(), Pane::Editor);
        assert!(
            app.splits.left.is_shown(),
            "FocusEditor must not hide the left pane"
        );
    }

    /// Review fix (plan WP5.S3, widened WP2): a dirty document with no live
    /// `db` binding (the default for an untitled draft) must never be
    /// silently discarded by the quit chord — `^C^C` (or `^D^D`) raises a
    /// `DirtyQuit` Guard rather than quitting or (WP2's own fix) merely
    /// closing.
    #[test]
    fn double_quit_chord_on_an_unpreserved_dirty_doc_raises_a_guard_instead_of_quitting() {
        let mut app = app();
        // Dirty is a content comparison now (plan WP1) — poking the render-
        // only cache directly would just be overwritten by `is_dirty_now`'s
        // re-derive, so diverge `saved_content` from the live buffer
        // instead, exactly like a real edit would.
        app.doc_mut(app.active)
            .expect("active doc exists")
            .saved_content = Arc::from("");
        assert!(app.active_doc().db.is_none(), "test setup: no db binding");

        let mut effects = Effects::default();
        handle_quit_key(&mut app, QuitKey::CtrlC, &mut effects);
        handle_quit_key(&mut app, QuitKey::CtrlC, &mut effects);

        assert!(
            !app.should_quit,
            "quit must not complete while unpreserved dirty work exists"
        );
        assert!(
            matches!(
                app.modal,
                Some(Modal::Guard(GuardPrompt {
                    kind: GuardKind::DirtyQuit,
                    ..
                }))
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
        // Genuinely dirty (plan WP1: a content comparison, not the cache) —
        // `is_dirty_now`'s re-derive would just overwrite a cache poke.
        app.doc_mut(app.active)
            .expect("active doc exists")
            .saved_content = Arc::from("");
        app.doc_mut(app.active).expect("active doc exists").db =
            Some(crate::db::DocDb::new(1, 0, true, 0));
        app.db = Some(live_db());

        let mut effects = Effects::default();
        handle_quit_key(&mut app, QuitKey::CtrlC, &mut effects);
        assert!(!app.should_quit, "the first press only arms the confirm");
        handle_quit_key(&mut app, QuitKey::CtrlC, &mut effects);
        assert!(app.should_quit, "the second matching press quits");
    }
}
