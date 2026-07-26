//! `App`: the Elm-style model. `update` is the ONLY writer of synchronous
//! state (CONSTITUTION §5.4: "mutate synchronous state directly in
//! `update`; a Cmd is exclusively for I/O that leaves the thread").

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rune_core::buffer::Buffer;
use rune_core::vfs::Vfs;
use rune_md::element::doc::ViewSnapshots;

use crate::commands::{clipboard, edit, nav};
use crate::editor::Editor;
use crate::keymap::{self, Command, KeyCode, KeyInput, Mods, QuitKey};
use crate::runtime::{Cmd, Effects, Msg};

/// The quit-confirm arm-to-quit window (plan Context, "Quit-confirm": "first
/// press arms + spawns 2s timer Cmd carrying gen").
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

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
        Msg::Paste(text) => {
            // Bracketed paste and pbpaste (`Msg::ClipboardRead` below) both
            // funnel through the same `handle_paste_content` (plan Gotchas:
            // "Bracketed paste vs pbpaste double-paste" — never handle one
            // event twice, never insert through two different paths).
            clipboard::handle_paste_content(app, &text);
        }
        Msg::ClipboardRead(text) => {
            clipboard::handle_paste_content(app, &text);
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
    // Hardcoded fast paths outside the resolver, exactly as Go
    // (`textedit/update.go:67-85`): Enter (mod 0) -> newline; Escape ->
    // collapse selection. Neither is a resolver-bound chord (plan Context,
    // "Keymap").
    if key.code == KeyCode::Enter && key.mods == Mods::NONE {
        edit::newline(app);
        return;
    }
    if key.code == KeyCode::Escape && key.mods == Mods::NONE {
        nav::escape(app);
        return;
    }

    let Some(command) = keymap::resolve(key) else {
        // Unmatched printable text -> insert fallthrough (plan Context,
        // "Hardcoded fast paths outside the resolver": `update.go:134-158`).
        // Ctrl/Alt/Super chords that reach here are simply unbound, never an
        // insert — every bound Ctrl/Alt/Super chord is already caught by
        // `keymap::resolve` above.
        if let KeyCode::Char(ch) = key.code
            && !key.mods.ctrl
            && !key.mods.alt
            && !key.mods.sup
            && is_insertable_key_char(ch)
        {
            edit::insert_char(app, ch);
        }
        return;
    };

    match command {
        Command::CharLeft => nav::char_left(app, false),
        Command::CharRight => nav::char_right(app, false),
        Command::LineUp => nav::line_up(app, false),
        Command::LineDown => nav::line_down(app, false),
        Command::WordLeft => nav::word_left(app, false),
        Command::WordRight => nav::word_right(app, false),
        Command::LineStart => nav::line_start(app, false),
        Command::LineEnd => nav::line_end(app, false),
        Command::PageUp => nav::page_up(app, false),
        Command::PageDown => nav::page_down(app, false),
        Command::SelectCharLeft => nav::char_left(app, true),
        Command::SelectCharRight => nav::char_right(app, true),
        Command::SelectLineUp => nav::line_up(app, true),
        Command::SelectLineDown => nav::line_down(app, true),
        Command::SelectWordLeft => nav::word_left(app, true),
        Command::SelectWordRight => nav::word_right(app, true),
        Command::SelectLineStart => nav::line_start(app, true),
        Command::SelectLineEnd => nav::line_end(app, true),
        Command::SelectPageUp => nav::page_up(app, true),
        Command::SelectPageDown => nav::page_down(app, true),
        Command::SelectAll => nav::select_all(app),
        Command::DeleteLeft => edit::delete_left(app),
        Command::DeleteRight => edit::delete_right(app),
        Command::Indent => edit::indent(app),
        Command::Outdent => edit::outdent(app),
        Command::Undo => edit::undo(app),
        Command::Redo => edit::redo(app),
        Command::Copy => clipboard::copy(app, effects),
        Command::Cut => clipboard::cut(app, effects),
        Command::Paste => clipboard::paste(effects),
        // Save is wired in WP9.
        Command::Save => {}
        Command::QuitConfirm => {
            // `resolve` only ever returns `QuitConfirm` when `key` is a
            // known quit chord (see `keymap::QuitKey::from_key`, the single
            // source of truth both functions route through).
            if let Some(quit_key) = QuitKey::from_key(key) {
                handle_quit_key(app, quit_key, effects);
            }
        }
    }
}

/// Guards the printable-insert fallthrough against control-byte leakage
/// (data-integrity fix, review finding F1). Go's equivalent gate is
/// `isPrintableChar` (`textedit.go:441-443`: `r >= ' ' && r <= '~'`), but
/// that gate applies ONLY to Go's SYNTHESIZED-from-`BaseCode` case
/// (`update.go:136-145`) — real decoded text (`msg.Text`, and everything
/// `Msg::Paste` carries here) flows unrestricted, including non-ASCII
/// (CJK, emoji). This crate's termina-backed `KeyCode::Char(char)` has no
/// such split: it is Go's `BaseCode` concept alone, never a separate
/// decoded-text stream, so a literal ASCII-only port would also block
/// genuine direct-keystroke Unicode entry Go itself allows unrestricted
/// (and which `tests/tui_edit.rs` requires). The hazard Go's gate actually
/// closes is narrower than "ASCII only": a raw C0 control byte or DEL
/// leaking through as `Char` with no modifier flag at all — the reported
/// case is a non-Kitty terminal's legacy encoding, where Ctrl+A IS the
/// literal SOH byte (no separate "this was a chord" signal survives
/// decoding) rather than a Kitty-protocol key report with an explicit
/// Ctrl modifier. Such a leaked byte can only ever be a single codepoint
/// in `0x00..=0x1F` or `0x7F` — ASCII's own control range — so excluding
/// `char::is_control()` (Unicode category Cc: `0x00..=0x1F` and
/// `0x7F..=0x9F`) closes that exact hazard without narrowing what a human
/// can actually type.
fn is_insertable_key_char(ch: char) -> bool {
    !ch.is_control()
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
        std::thread::sleep(CONFIRM_TIMEOUT);
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

    /// Regression for F1: a raw C0 control byte or DEL arriving as
    /// `KeyCode::Char` with NO modifier flag at all (the non-Kitty legacy-
    /// terminal degradation path, where Ctrl+A IS the literal SOH byte)
    /// must never reach the buffer.
    #[test]
    fn control_bytes_with_no_modifier_are_never_inserted() {
        let mut app = test_app();
        let before = app.editor.buffer.content().to_string();

        for raw in ['\u{1}', '\u{7f}', '\u{1b}'] {
            let mut effects = Effects::default();
            update(
                &mut app,
                Msg::Key(key(KeyCode::Char(raw), Mods::NONE)),
                &mut effects,
            );
        }

        assert_eq!(
            app.editor.buffer.content(),
            before,
            "a raw control byte must never be inserted into the document"
        );
    }

    #[test]
    fn printable_ascii_and_unicode_chars_are_still_insertable() {
        let mut app = test_app();
        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::Key(key(KeyCode::Char('汉'), Mods::NONE)),
            &mut effects,
        );
        assert!(
            app.editor.buffer.content().contains('汉'),
            "genuine Unicode text entry must not be blocked by the control-byte guard"
        );
    }
}
