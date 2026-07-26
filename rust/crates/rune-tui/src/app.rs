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
        Command::Save => trigger_save(app, effects),
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

/// `super+s` (WP9, plan Context "Save"): guarded by the in-flight flag (a
/// second `super+s` before the first save's `Cmd` reports back is a no-op —
/// there is exactly one save `Cmd` in flight at a time, so its eventual
/// `Msg::SaveDone` can never be ambiguous about which attempt it answers)
/// and by `version != saved_version` (nothing to persist otherwise). Clones
/// the buffer's bytes and its CURRENT version onto the `Cmd`'s own thread —
/// never the buffer itself, never the `Vfs` call site's shared state — so
/// the actual `save_atomic` I/O happens entirely off the main thread
/// (§5.4). `version` rides along on `Msg::SaveDone` so `update`'s handler
/// can tell whether further edits landed while this save was in flight (see
/// its docs) rather than blindly trusting "a save just finished" to mean
/// "the buffer is now clean".
fn trigger_save(app: &mut App, effects: &mut Effects) {
    if app.save_in_flight {
        return;
    }
    let version = app.editor.buffer.version();
    if version == app.saved_version {
        return;
    }
    let Some(path) = app.file_path.clone() else {
        app.status_message =
            Some("no file to save \u{2014} rune was opened without a path".to_string());
        return;
    };

    app.save_in_flight = true;
    let bytes = app.editor.buffer.content().as_bytes().to_vec();
    let vfs = Arc::clone(&app.vfs);
    effects.cmds.push(save_cmd(vfs, path, bytes, version));
}

/// The off-thread save I/O itself: `vfs.save_atomic` (§1.4.1's durable
/// temp-write + atomic publish, or `Mem`'s test double) writes EXACTLY
/// `bytes` — §1.4.5 byte-verbatim, no normalization anywhere on this path.
fn save_cmd(vfs: Arc<dyn Vfs + Send + Sync>, path: PathBuf, bytes: Vec<u8>, version: u64) -> Cmd {
    Box::new(move || {
        let result = vfs.save_atomic(&path, &bytes).map_err(|e| e.to_string());
        Some(Msg::SaveDone { version, result })
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::keymap::{KeyCode, Mods};
    use rune_core::vfs::{Disk, Mem, Vfs};

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

    fn save_key() -> KeyInput {
        key(
            KeyCode::Char('s'),
            Mods {
                sup: true,
                ..Mods::NONE
            },
        )
    }

    /// Presses `super+s` through the real `update` and returns the
    /// `Effects` it produced — the caller drives `effects.cmds` to
    /// completion itself via `settle_cmds` (headless: this crate's `Cmd` is
    /// a plain `FnOnce`, no real thread or terminal needed to run one).
    fn press_save(app: &mut App) -> Effects {
        let mut effects = Effects::default();
        update(app, Msg::Key(save_key()), &mut effects);
        effects
    }

    /// Runs every `Cmd` in `effects` synchronously and feeds each resulting
    /// `Msg` back through `update`, recursively settling whatever new
    /// `Effects` that produces — the headless stand-in for `runtime::run`'s
    /// spawn-then-`recv` loop.
    fn settle_cmds(app: &mut App, effects: Effects) {
        for cmd in effects.cmds {
            if let Some(msg) = cmd() {
                let mut next = Effects::default();
                update(app, msg, &mut next);
                settle_cmds(app, next);
            }
        }
    }

    #[test]
    fn save_persists_exact_bytes_for_crlf_bom_and_no_trailing_newline_fixtures() {
        for content in ["a\r\nb\r\n", "\u{feff}hello", "no trailing newline"] {
            let vfs = Arc::new(Mem::new());
            let path = PathBuf::from("/doc.md");
            let mut app = App::new(
                Buffer::new(content),
                Some(path.clone()),
                Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
            );
            // The buffer as freshly loaded IS the saved state (App::new sets
            // `saved_version = buffer.version()`) — force it dirty without
            // touching the CONTENT, so `super+s` actually has something to
            // persist and the assertion below is exercising the real write
            // path, not a same-content no-op.
            app.saved_version = 0;

            let effects = press_save(&mut app);
            assert_eq!(effects.cmds.len(), 1, "one save Cmd must be spawned");
            settle_cmds(&mut app, effects);

            let saved = vfs.read(&path).expect("save must have written the file");
            assert_eq!(
                saved,
                content.as_bytes(),
                "saved bytes must be byte-identical to the buffer, verbatim"
            );
            assert!(!app.is_dirty());
        }
    }

    #[test]
    fn save_failure_surfaces_a_status_error_and_keeps_dirty() {
        let vfs = Arc::new(Mem::new());
        vfs.fail_next_save(std::io::ErrorKind::Other);
        let path = PathBuf::from("/doc.md");
        let mut app = App::new(
            Buffer::new("hello"),
            Some(path),
            Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
        );
        app.saved_version = 0; // force dirty — see the byte-exact test's comment

        let effects = press_save(&mut app);
        settle_cmds(&mut app, effects);

        assert!(app.is_dirty());
        assert!(
            app.status_message.is_some(),
            "a failed save must surface a status-line error"
        );
    }

    #[test]
    fn a_second_save_press_while_one_is_in_flight_is_a_no_op() {
        let mut app = App::new(
            Buffer::new("hello"),
            Some(PathBuf::from("/doc.md")),
            Arc::new(Mem::new()),
        );
        app.editor.buffer = app.editor.buffer.insert(0, "x"); // makes it dirty

        let effects = press_save(&mut app);
        assert_eq!(effects.cmds.len(), 1);
        assert!(app.save_in_flight);

        // A second press before the first save's Cmd has run must not spawn
        // a second Cmd.
        let effects2 = press_save(&mut app);
        assert!(
            effects2.cmds.is_empty(),
            "a save already in flight must not spawn a second save Cmd"
        );
        assert!(app.save_in_flight);
    }

    #[test]
    fn an_edit_during_a_save_keeps_the_buffer_dirty_once_the_save_completes() {
        let vfs = Arc::new(Mem::new());
        let path = PathBuf::from("/doc.md");
        let mut app = App::new(
            Buffer::new("hello"),
            Some(path),
            Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
        );
        app.saved_version = 0; // force dirty — see the byte-exact test's comment

        let effects = press_save(&mut app); // captures the pre-edit version
        assert_eq!(effects.cmds.len(), 1);

        // An edit lands while the save Cmd hasn't reported back yet.
        edit::insert_char(&mut app, '!');
        let after_edit_version = app.editor.buffer.version();

        settle_cmds(&mut app, effects); // delivers SaveDone for the OLD version

        assert!(
            app.saved_version < after_edit_version,
            "SaveDone must only advance saved_version to the version IT saved, \
             not the buffer's current (post-edit) version"
        );
        assert!(
            app.is_dirty(),
            "an edit made during the in-flight save must leave the buffer dirty \
             once that save completes"
        );
    }

    #[test]
    fn saving_a_path_that_does_not_exist_on_disk_creates_it_via_the_excl_path() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rune-wp9-excl-{}-{n}.md", std::process::id()));
        let _ = std::fs::remove_file(&path); // in case a prior run left it behind
        assert!(!path.exists(), "the fixture path must not exist yet");

        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Disk);
        let mut app = App::new(Buffer::new("brand new file\n"), Some(path.clone()), vfs);
        app.saved_version = 0; // force dirty — see the byte-exact test's comment

        let effects = press_save(&mut app);
        settle_cmds(&mut app, effects);

        assert!(!app.is_dirty());
        let saved = std::fs::read(&path).expect("save must have created the file on disk");
        assert_eq!(saved, b"brand new file\n");

        let _ = std::fs::remove_file(&path);
    }
}
