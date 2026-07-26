//! `App`: the Elm-style model. `update` is the ONLY writer of synchronous
//! state (CONSTITUTION §5.4: "mutate synchronous state directly in
//! `update`; a Cmd is exclusively for I/O that leaves the thread").

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rune_core::buffer::Buffer;
use rune_core::vfs::Vfs;
use rune_md::element::doc::ViewSnapshots;

use crate::editor::Editor;
use crate::keymap::{self, Command, KeyInput, QuitKey};
use crate::runtime::{Cmd, Effects, Msg};

/// The whole editor model: the single editing pane (Phase 1 is one file, one
/// pane), file identity, the injected `Vfs` save target, and app-wide UI
/// state (status message, quit-confirm arming) that doesn't belong to any
/// one editing pane.
pub struct App {
    pub editor: Editor,
    pub file_path: Option<PathBuf>,
    pub vfs: Arc<dyn Vfs + Send + Sync>,
    pub saved_version: u64,
    pub save_in_flight: bool,
    pub status_message: Option<String>,
    /// The armed quit chord and its timer generation — `None` when no quit
    /// is pending. Stale `ConfirmTimeout` generations are ignored (plan
    /// Context, "Quit-confirm").
    pub pending_quit: Option<(QuitKey, u32)>,
    next_quit_gen: u32,
    pub should_quit: bool,
    /// The most recent display-pipeline snapshot, cached by `sync_view` for
    /// `render::draw` to blit. `None` only before the first sync.
    pub view: Option<ViewSnapshots>,
}

impl App {
    pub fn new(buffer: Buffer, file_path: Option<PathBuf>, vfs: Arc<dyn Vfs + Send + Sync>) -> App {
        let saved_version = buffer.version();
        App {
            editor: Editor::new(buffer),
            file_path,
            vfs,
            saved_version,
            save_in_flight: false,
            status_message: None,
            pending_quit: None,
            next_quit_gen: 0,
            should_quit: false,
            view: None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.editor.buffer.version() != self.saved_version
    }

    pub fn file_name(&self) -> &str {
        self.file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("[No Name]")
    }

    /// Re-runs the display pipeline and caches the result for `render::draw`.
    /// Safe to call more than once per message batch — see `Editor::sync`'s
    /// docs.
    pub fn sync_view(&mut self) {
        self.view = Some(self.editor.sync());
    }
}

/// The ONLY writer of `App` state (§5.4). `effects` accumulates I/O for the
/// runtime loop to perform after the whole message batch is applied:
/// `effects.raw` for OSC 52 (drained by the main loop, never a `Cmd` — plan
/// Gotchas, "Cmds must never touch the terminal"), `effects.cmds` for
/// off-thread work (save, pbpaste, the quit-confirm timer).
pub fn update(app: &mut App, msg: Msg, effects: &mut Effects) {
    match msg {
        Msg::Key(key) => handle_key(app, key, effects),
        Msg::Resize(width, height) => {
            app.editor
                .viewport
                .set_size(width, height.saturating_sub(1));
        }
        Msg::Paste(_text) => {
            // `handle_paste_content` is wired in WP8; WP5 has no editing
            // yet, so a paste is a no-op placeholder.
        }
        Msg::ClipboardRead(_text) => {
            // Wired alongside paste handling in WP8.
        }
        Msg::SaveDone { version, result } => {
            app.save_in_flight = false;
            match result {
                Ok(()) => {
                    if version > app.saved_version {
                        app.saved_version = version;
                    }
                    app.status_message = None;
                }
                Err(e) => {
                    app.status_message = Some(format!("save failed: {e}"));
                }
            }
        }
        Msg::ConfirmTimeout { generation } => {
            if let Some((_, pending_gen)) = app.pending_quit
                && pending_gen == generation
            {
                app.pending_quit = None;
            }
            // A stale generation (the user already quit-confirmed or
            // re-armed with a new chord since) is ignored.
        }
        Msg::Error(e) => {
            app.status_message = Some(e);
        }
        Msg::Quit => {
            app.should_quit = true;
        }
    }
}

fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) {
    let Some(command) = keymap::resolve(key) else {
        // WP7 wires the printable-insert fallthrough for unresolved keys;
        // WP5 has no editing yet, so an unbound key is simply ignored.
        return;
    };

    // Movement/selection/editing/clipboard/undo/save commands are acted on
    // starting WP6/7/8/9 (plan: "movement commands may no-op until WP6").
    // The resolver already covers the full chord table; only this dispatch
    // grows in later WPs.
    if command == Command::QuitConfirm {
        // `resolve` only ever returns `QuitConfirm` when `key` is a known
        // quit chord (see `keymap::QuitKey::from_key`, the single source of
        // truth both functions route through).
        if let Some(quit_key) = QuitKey::from_key(key) {
            handle_quit_key(app, quit_key, effects);
        }
    }
}

/// Port of the quit-confirm state machine (plan Context, "Quit-confirm",
/// mirroring `footer.go:230-237`): the SAME chord pressed twice quits;
/// pressing a quit chord while a DIFFERENT quit chord is pending re-arms
/// with the new chord and a fresh generation, restarting the 2s window.
fn handle_quit_key(app: &mut App, key: QuitKey, effects: &mut Effects) {
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
/// (superseded by a second press or a re-arm) is ignored on arrival (plan
/// Context, "Quit-confirm"). Genuine wall-clock pacing for a real UI
/// feature — not a test-ordering hack — so `std::thread::sleep` here is the
/// correct primitive (this Cmd runs on its own dedicated thread by runtime
/// design, never blocking the main loop).
fn quit_confirm_timeout_cmd(generation: u32) -> Cmd {
    Box::new(move || {
        std::thread::sleep(Duration::from_secs(2));
        Some(Msg::ConfirmTimeout { generation })
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::keymap::{KeyCode, Mods};
    use rune_core::vfs::Mem;

    fn test_app() -> App {
        App::new(Buffer::new("hello"), None, Arc::new(Mem::new()))
    }

    fn key(code: KeyCode, mods: Mods) -> KeyInput {
        KeyInput { code, mods }
    }

    #[test]
    fn first_quit_press_arms_and_spawns_a_timer_cmd_without_quitting() {
        let mut app = test_app();
        let mut effects = Effects::default();
        let ctrl_c = key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );

        update(&mut app, Msg::Key(ctrl_c), &mut effects);

        assert!(!app.should_quit);
        assert_eq!(app.pending_quit, Some((QuitKey::CtrlC, 0)));
        assert_eq!(effects.cmds.len(), 1);
    }

    #[test]
    fn same_chord_twice_quits() {
        let mut app = test_app();
        let ctrl_c = key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );

        let mut effects = Effects::default();
        update(&mut app, Msg::Key(ctrl_c), &mut effects);
        assert!(!app.should_quit);

        let mut effects = Effects::default();
        update(&mut app, Msg::Key(ctrl_c), &mut effects);
        assert!(app.should_quit);
    }

    #[test]
    fn different_quit_chord_re_arms_instead_of_quitting() {
        let mut app = test_app();
        let ctrl_c = key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );
        let ctrl_alt_d = key(
            KeyCode::Char('d'),
            Mods {
                ctrl: true,
                alt: true,
                ..Mods::NONE
            },
        );

        let mut effects = Effects::default();
        update(&mut app, Msg::Key(ctrl_c), &mut effects);
        assert_eq!(app.pending_quit, Some((QuitKey::CtrlC, 0)));

        let mut effects = Effects::default();
        update(&mut app, Msg::Key(ctrl_alt_d), &mut effects);
        assert!(!app.should_quit, "a different quit chord must not quit");
        assert_eq!(app.pending_quit, Some((QuitKey::CtrlAltD, 1)));
    }

    #[test]
    fn matching_confirm_timeout_clears_pending_quit() {
        let mut app = test_app();
        let ctrl_c = key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );
        let mut effects = Effects::default();
        update(&mut app, Msg::Key(ctrl_c), &mut effects);
        assert_eq!(app.pending_quit, Some((QuitKey::CtrlC, 0)));

        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::ConfirmTimeout { generation: 0 },
            &mut effects,
        );
        assert_eq!(app.pending_quit, None);
        assert!(!app.should_quit);
    }

    #[test]
    fn stale_confirm_timeout_is_ignored() {
        let mut app = test_app();
        let ctrl_c = key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );
        let ctrl_alt_d = key(
            KeyCode::Char('d'),
            Mods {
                ctrl: true,
                alt: true,
                ..Mods::NONE
            },
        );
        let mut effects = Effects::default();
        update(&mut app, Msg::Key(ctrl_c), &mut effects); // generation 0
        let mut effects2 = Effects::default();
        update(&mut app, Msg::Key(ctrl_alt_d), &mut effects2); // re-arms, generation 1
        assert_eq!(app.pending_quit, Some((QuitKey::CtrlAltD, 1)));

        // The stale generation-0 timeout must not clear the generation-1 pending quit.
        let mut effects3 = Effects::default();
        update(
            &mut app,
            Msg::ConfirmTimeout { generation: 0 },
            &mut effects3,
        );
        assert_eq!(app.pending_quit, Some((QuitKey::CtrlAltD, 1)));
    }

    #[test]
    fn save_done_ok_advances_saved_version_and_clears_status() {
        let mut app = test_app();
        app.status_message = Some("save failed: oops".to_string());
        let version = app.editor.buffer.version();
        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::SaveDone {
                version,
                result: Ok(()),
            },
            &mut effects,
        );
        assert_eq!(app.saved_version, version);
        assert!(app.status_message.is_none());
    }

    #[test]
    fn save_done_err_surfaces_status_and_keeps_dirty() {
        let mut app = test_app();
        app.editor.buffer = app.editor.buffer.insert(0, "x");
        let before_saved = app.saved_version;
        let version = app.editor.buffer.version();
        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::SaveDone {
                version,
                result: Err("disk full".to_string()),
            },
            &mut effects,
        );
        assert_eq!(app.saved_version, before_saved);
        assert!(app.is_dirty());
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("disk full"))
        );
    }

    #[test]
    fn resize_sets_viewport_size_reserving_the_status_row() {
        let mut app = test_app();
        let mut effects = Effects::default();
        update(&mut app, Msg::Resize(80, 24), &mut effects);
        assert_eq!(app.editor.viewport.width, 80);
        assert_eq!(app.editor.viewport.height, 23);
    }
}
