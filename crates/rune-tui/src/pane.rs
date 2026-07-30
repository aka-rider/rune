//! `Pane` — the focus discriminant (plan Context, decision 7: "Pane enum =
//! focus discriminant only" — no trait objects; `Explorer`/`Tabs`'s own
//! state lands in plain named `App` fields in WP4/WP5). Extracted out of
//! `app.rs` to keep it under the §1.6 budget (plan WP2 Rules: "extract to
//! pane.rs ... as needed") — `handle_global_command` lives here too since
//! it's the sole reader/writer of `App::focus`/the left column's `Split`
//! state outside `app.rs` itself.

use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
use crate::banner::{self, GuardKind, GuardPrompt, Modal};
use crate::document::DocumentId;
use crate::explorer;
use crate::keymap::{GlobalCommand, QuitKey};
use crate::runtime::{Cmd, CmdKind, DirCause, Effects, Msg, load_dir_cmd};
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
    // never silently discarded. A no-op when the title isn't focused.
    crate::title::finalize_if_focused(app, effects);

    match cmd {
        GlobalCommand::FocusExplorer => {
            // Always exposes and focuses the Explorer — never hides it, so
            // the command a user reaches for to SEE the Explorer can never
            // instead take it away (mirrors the Go reference's own
            // show-plus-focus contract, and `FocusTabs`'s below).
            app.splits.left.show();
            app.splits.explorer.show();
            app.focus = Pane::Explorer;
            // The Explorer's very first load (plan WP4.S4): "empty and not
            // already loading" is the no-shadow-state stand-in for "never
            // loaded" — `Explorer`'s exact field list (`explorer.rs`) has
            // no separate `loaded` flag, and a genuinely-empty directory
            // re-triggering this on a later focus is a harmless no-op
            // reload, not an incorrect state.
            if app.splits.left.is_shown()
                && app.explorer.entries.is_empty()
                && !app.explorer.loading
            {
                let root = explorer::initial_root(app);
                app.explorer.loading = true;
                app.explorer.request_generation = app.explorer.request_generation.wrapping_add(1);
                let generation = app.explorer.request_generation;
                let vfs = Arc::clone(&app.vfs);
                effects
                    .cmds
                    .push(load_dir_cmd(vfs, root, DirCause::Nav, generation));
            }
        }
        GlobalCommand::FocusEditor => app.focus = Pane::Editor,
        // Reseed from the document that is actually showing, every time:
        // the field must never present a stale name from a previous
        // document or a previously abandoned edit (no shadow state).
        GlobalCommand::FocusTitle => focus_title(app),
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
            app.focus = Pane::Tabs;
        }
        GlobalCommand::CollapseLeft => {
            app.splits.left.hide();
            if matches!(app.focus, Pane::Explorer | Pane::Tabs) {
                app.focus = Pane::Editor;
            }
        }
        GlobalCommand::Save => save::trigger_save(app, app.active, effects),
        // WP7.S2: mints/toggles the generated Help virtual document — a
        // direct, same-tick call (decision 10), no I/O involved.
        GlobalCommand::Help => crate::workspace::toggle_help(app),
        GlobalCommand::QuitChord(key) => handle_quit_key(app, key, effects),
        // Routes through the one close chokepoint regardless of which pane
        // held focus when `^w` was pressed, so a dirty document still arms
        // its Guard exactly like the Tabs-pane-local close it replaces.
        GlobalCommand::CloseFile => crate::workspace::request_close(app, app.active),
        // Out-of-range is a silent no-op — the same tolerance `TabsCommand::
        // Select`'s own `.get` already had for a cursor past the end.
        GlobalCommand::TabSwitch(idx) => {
            if let Some(&id) = app.tabs.order.get(idx) {
                crate::workspace::switch_to(app, id);
            }
        }
    }
}

/// Focuses the title field, reseeding it from the active document's own
/// stem and landing the cursor at the end. The single entry point for
/// gaining title focus — `^r` and the Up-at-editor-top gesture both route
/// here, so the seed can never be skipped by one of them.
pub(crate) fn focus_title(app: &mut App) {
    let stem = crate::title::stem_for(app.active_doc());
    app.title.seed(&stem);
    app.focus = Pane::Title;
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
    // work with no journal to recover it from. Raise the same Guard the
    // ordinary close path (`workspace::request_close`) uses instead of
    // arming or completing quit; the user resolves it exactly like any
    // other dirty-close prompt, then presses the quit chord again.
    if let Some(doc) = first_unpreserved_dirty_doc(app) {
        let _ = banner::set_modal(
            app,
            Modal::Guard(GuardPrompt {
                doc,
                kind: GuardKind::DirtyClose,
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

/// The first (lowest `DocumentId`) open document that is both dirty and has
/// no live recovery-store binding — quit preserves through the durable
/// journal, so a dirty document without one is the exact case `handle_quit_
/// key`'s Guard gate exists for. Deterministic ordering (`documents` is a
/// `BTreeMap`) rather than "whichever `HashMap` bucket happens to iterate
/// first" — repeated presses always raise the Guard for the same document
/// until it's resolved.
fn first_unpreserved_dirty_doc(app: &App) -> Option<DocumentId> {
    app.documents
        .iter()
        .find(|(_, doc)| doc.is_dirty() && doc.db.is_none())
        .map(|(id, _)| *id)
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
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app() -> App {
        App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
    }

    #[test]
    fn focus_explorer_shows_the_left_pane_and_focuses_it() {
        let mut app = app();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::FocusExplorer, &mut effects);
        assert!(app.splits.left.is_shown());
        assert_eq!(app.focus, Pane::Explorer);
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
        assert_eq!(app.focus, Pane::Explorer);
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
        assert_eq!(app.focus, Pane::Editor);
    }

    #[test]
    fn focus_editor_returns_focus_regardless_of_the_left_columns_visibility() {
        let mut app = app();
        app.focus = Pane::Explorer;
        app.splits.left.show();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::FocusEditor, &mut effects);
        assert_eq!(app.focus, Pane::Editor);
        assert!(
            app.splits.left.is_shown(),
            "FocusEditor must not hide the left pane"
        );
    }

    /// Review fix (plan WP5.S3): a dirty document with no live `db` binding
    /// (the default for an untitled draft) must never be silently discarded
    /// by the quit chord — `^C^C` (or `^D^D`) raises the same dirty-close
    /// Guard `workspace::request_close` uses instead of quitting.
    #[test]
    fn double_quit_chord_on_an_unpreserved_dirty_doc_raises_a_guard_instead_of_quitting() {
        let mut app = app();
        app.doc_mut(app.active)
            .expect("active doc exists")
            .is_dirty_cached = true;
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
                    kind: GuardKind::DirtyClose,
                    ..
                }))
            ),
            "expected a DirtyClose Guard prompt to be raised"
        );
    }

    /// The converse: a dirty document that IS preserved (has a live `db`
    /// binding) doesn't trip the new gate — the ordinary two-press
    /// quit-confirm still works exactly as before.
    #[test]
    fn double_quit_chord_on_a_preserved_dirty_doc_still_quits() {
        let mut app = app();
        app.doc_mut(app.active)
            .expect("active doc exists")
            .is_dirty_cached = true;
        app.doc_mut(app.active).expect("active doc exists").db =
            Some(crate::db::DocDb::new(1, 0, true, 0));

        let mut effects = Effects::default();
        handle_quit_key(&mut app, QuitKey::CtrlC, &mut effects);
        assert!(!app.should_quit, "the first press only arms the confirm");
        handle_quit_key(&mut app, QuitKey::CtrlC, &mut effects);
        assert!(app.should_quit, "the second matching press quits");
    }
}
