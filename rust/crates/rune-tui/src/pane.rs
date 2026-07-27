//! `Pane` — the focus discriminant (plan Context, decision 7: "Pane enum =
//! focus discriminant only" — no trait objects; `Explorer`/`Tabs`'s own
//! state lands in plain named `App` fields in WP4/WP5). Extracted out of
//! `app.rs` to keep it under the §1.6 budget (plan WP2 Rules: "extract to
//! pane.rs ... as needed") — `handle_global_command` lives here too since
//! it's the sole reader/writer of `App::focus`/`App::left_visible` outside
//! `app.rs` itself.

use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
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
}

/// Stage 2 of the four-stage key pipeline (plan Context, decision 8,
/// `app::handle_key`): every `GlobalCommand` fires regardless of which pane
/// currently has focus — the quit chords and Save in particular must keep
/// working while the Explorer/Tabs stub panes own it (plan WP2.S4).
pub(crate) fn handle_global_command(app: &mut App, cmd: GlobalCommand, effects: &mut Effects) {
    match cmd {
        GlobalCommand::ToggleExplorer => {
            app.left_visible = !app.left_visible;
            app.focus = if app.left_visible {
                Pane::Explorer
            } else {
                Pane::Editor
            };
            // The Explorer's very first load (plan WP4.S4): "empty and not
            // already loading" is the no-shadow-state stand-in for "never
            // loaded" — `Explorer`'s exact field list (`explorer.rs`) has
            // no separate `loaded` flag, and a genuinely-empty directory
            // re-triggering this on a later toggle is a harmless no-op
            // reload, not an incorrect state.
            if app.left_visible && app.explorer.entries.is_empty() && !app.explorer.loading {
                let root = explorer::initial_root(app);
                app.explorer.loading = true;
                let vfs = Arc::clone(&app.vfs);
                effects.cmds.push(load_dir_cmd(vfs, root, DirCause::Nav));
            }
        }
        GlobalCommand::FocusEditor => app.focus = Pane::Editor,
        GlobalCommand::Save => save::trigger_save(app, app.active, effects),
        // WP7 mints the generated Help document; until then F1 is bound
        // but inert rather than unbound (plan WP2.S4: "a no-op stub with a
        // `// WP7` comment").
        GlobalCommand::Help => {}
        GlobalCommand::QuitChord(key) => handle_quit_key(app, key, effects),
    }
}

/// Port of the quit-confirm state machine (plan Context, "Quit-confirm",
/// mirroring Go `footer.go:230-237`): the SAME chord pressed twice quits;
/// pressing a quit chord while a DIFFERENT one is pending re-arms with the
/// new chord and a fresh generation, restarting the 2s window. `pub(crate)`
/// — WP2 moved this out of `app.rs` (§1.6 budget); `handle_global_command`
/// above is its only caller now that quit chords resolve at the global
/// pipeline stage.
pub(crate) fn handle_quit_key(app: &mut App, key: QuitKey, effects: &mut Effects) {
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
    fn toggle_explorer_shows_the_left_pane_and_focuses_it() {
        let mut app = app();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::ToggleExplorer, &mut effects);
        assert!(app.left_visible);
        assert_eq!(app.focus, Pane::Explorer);
    }

    #[test]
    fn toggling_explorer_twice_hides_it_and_refocuses_the_editor() {
        let mut app = app();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::ToggleExplorer, &mut effects);
        handle_global_command(&mut app, GlobalCommand::ToggleExplorer, &mut effects);
        assert!(!app.left_visible);
        assert_eq!(app.focus, Pane::Editor);
    }

    #[test]
    fn focus_editor_returns_focus_regardless_of_left_visible() {
        let mut app = app();
        app.focus = Pane::Explorer;
        app.left_visible = true;
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::FocusEditor, &mut effects);
        assert_eq!(app.focus, Pane::Editor);
        assert!(app.left_visible, "FocusEditor must not hide the left pane");
    }
}
