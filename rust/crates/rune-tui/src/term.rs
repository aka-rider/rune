//! RAII terminal guard (plan Context, "Msg/Cmd runtime" / "Keymap"): enters
//! raw mode, the alternate screen, Kitty keyboard enhancement flags, and
//! bracketed paste on construction; `Drop` restores all of it. `Drop` runs
//! during unwind, before `catch_unwind` traps a panic in `rune-cli`'s
//! `main` — the halt path CONSTITUTION §1.3 requires (panic = "abort" is
//! forbidden precisely so this restore can run).
//!
//! Owns the one `termina::Terminal` for the process: main-thread-only by
//! construction (plan Gotchas: "Cmds must never touch the terminal" —
//! termina's `Terminal` is `io::Write` on `&mut self`, no documented
//! cross-thread write support).
//!
//! Cooked-mode restoration is NOT this module's job: `termina::UnixTerminal`
//! already runs `enter_cooked_mode()` unconditionally from its own `Drop`
//! (`termina-0.3.3/src/terminal/unix.rs:236-242` — the skip condition,
//! `has_panic_hook && thread::panicking()`, never applies here since this
//! crate never installs a termina panic hook), so `Guard::drop` only needs
//! to reverse the escape sequences it wrote by hand (alt screen, bracketed
//! paste, Kitty flags, cursor visibility) — it never reaches into the
//! wrapped `PlatformTerminal` at all, so no `unstable-backend-writer`
//! feature is needed.

use std::io::{self, Write};

use ratatui::Terminal as RtTerminal;
use ratatui::backend::TerminaBackend;
use termina::Terminal as _;
use termina::escape::csi::{
    Csi, DecPrivateMode, DecPrivateModeCode, Keyboard, KittyKeyboardFlags, Mode,
};
use termina::{EventReader, PlatformTerminal};

/// Kitty keyboard flags this app requests: disambiguate escape codes (tells
/// `Tab` apart from `ctrl+i`, etc.) and report alternate keys (needed for
/// `super+...` chords). A terminal without Kitty support ignores this CSI
/// outright — the ctrl alternates in the keymap table are the fallback
/// (plan Gotchas: "don't treat their absence as a bug").
fn keyboard_flags() -> KittyKeyboardFlags {
    KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES | KittyKeyboardFlags::REPORT_ALTERNATE_KEYS
}

/// `Guard::new` pushes exactly one Kitty keyboard flags entry
/// (`Keyboard::PushFlags`); `Drop` pops exactly that one entry back off —
/// the two calls are a pair, kept as a named constant so the relationship
/// is not two independently-chosen literals that could drift apart.
const KITTY_FLAGS_PUSHED: u8 = 1;

fn dec_set(code: DecPrivateModeCode) -> Csi {
    Csi::Mode(Mode::SetDecPrivateMode(DecPrivateMode::Code(code)))
}

fn dec_reset(code: DecPrivateModeCode) -> Csi {
    Csi::Mode(Mode::ResetDecPrivateMode(DecPrivateMode::Code(code)))
}

/// The terminal lifecycle guard. Holds the ratatui `Terminal` (which owns
/// the `termina::Terminal` by value through `TerminaBackend`) plus a cloned
/// `EventReader` handed to the input-reader thread — `EventReader` is
/// `Clone`+`Send` and independent of the backend's ownership of the
/// underlying terminal handle.
pub struct Guard {
    terminal: RtTerminal<TerminaBackend<PlatformTerminal>>,
    events: EventReader,
}

impl Guard {
    /// Every fallible step that happens BEFORE `Guard` exists as a value
    /// (`PlatformTerminal::new`, `enter_raw_mode`, `RtTerminal::new`,
    /// `hide_cursor`) writes nothing this module would need to reverse by
    /// hand — raw-mode restoration is `termina`'s own `Drop`'s job (see
    /// module docs), and none of those steps enters the alternate screen or
    /// touches bracketed paste / Kitty flags. Only `enter_app_mode`, called
    /// once `self` is a real `Guard`, writes those escapes — so ANY error
    /// on ANY path through this constructor either happens before those
    /// escapes are written (nothing to reverse) or happens after `self`
    /// exists (its `Drop` already runs on the early return and reverses
    /// whatever did get written). No path can leave the terminal stuck in
    /// the alternate screen with no guard alive to restore it.
    pub fn new() -> io::Result<Guard> {
        let mut output = PlatformTerminal::new()?;
        output.enter_raw_mode()?;
        let events = output.event_reader();

        let backend = TerminaBackend::new(output);
        let terminal = RtTerminal::new(backend)?;

        let mut guard = Guard { terminal, events };
        guard.enter_app_mode()?;
        Ok(guard)
    }

    /// Enables the alternate screen, bracketed paste, and Kitty keyboard
    /// flags, and hides the terminal cursor (render.rs draws the caret
    /// itself as a styled cell — plan Context, "Cell model": "Terminal
    /// cursor hidden; caret drawn by us (Go parity)"). Only ever called
    /// from `new`, on an already-constructed `Guard` — see its docs for why
    /// that ordering matters.
    fn enter_app_mode(&mut self) -> io::Result<()> {
        let backend = self.terminal.backend_mut();
        write!(
            backend,
            "{}{}{}",
            dec_set(DecPrivateModeCode::ClearAndEnableAlternateScreen),
            dec_set(DecPrivateModeCode::BracketedPaste),
            Csi::Keyboard(Keyboard::PushFlags(keyboard_flags())),
        )?;
        backend.flush()?;
        self.terminal.hide_cursor()
    }

    /// A clone of the event reader, handed to the input-reader thread.
    pub fn event_reader(&self) -> EventReader {
        self.events.clone()
    }

    pub fn size(&self) -> io::Result<(u16, u16)> {
        let size = self.terminal.size()?;
        Ok((size.width, size.height))
    }

    pub fn draw(&mut self, f: impl FnOnce(&mut ratatui::Frame)) -> io::Result<()> {
        self.terminal.draw(f)?;
        Ok(())
    }

    /// The ONLY path raw escape bytes (OSC 52) reach the terminal — called
    /// exclusively from the main loop between the message batch and the
    /// next draw (plan Gotchas: "Cmds must never touch the terminal").
    pub fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        let backend = self.terminal.backend_mut();
        backend.write_all(bytes)?;
        backend.flush()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // §1.3: halt, never panic — every step here is best-effort. A
        // failure restoring one escape sequence must not prevent the rest
        // from running (a half-restored terminal is still better than
        // none). `termina::UnixTerminal`'s own `Drop` restores cooked mode
        // once `self.terminal` (and the `PlatformTerminal` it owns) is
        // dropped after this — see module docs.
        let backend = self.terminal.backend_mut();
        let _ = write!(
            backend,
            "{}{}{}{}",
            Csi::Keyboard(Keyboard::PopFlags(KITTY_FLAGS_PUSHED)),
            dec_reset(DecPrivateModeCode::BracketedPaste),
            dec_reset(DecPrivateModeCode::ClearAndEnableAlternateScreen),
            dec_set(DecPrivateModeCode::ShowCursor),
        );
        let _ = backend.flush();
    }
}
