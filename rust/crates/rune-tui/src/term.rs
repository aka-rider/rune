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
    pub fn new() -> io::Result<Guard> {
        let mut output = PlatformTerminal::new()?;
        output.enter_raw_mode()?;
        let events = output.event_reader();

        write!(
            output,
            "{}{}{}",
            dec_set(DecPrivateModeCode::ClearAndEnableAlternateScreen),
            dec_set(DecPrivateModeCode::BracketedPaste),
            Csi::Keyboard(Keyboard::PushFlags(keyboard_flags())),
        )?;
        output.flush()?;

        let backend = TerminaBackend::new(output);
        let mut terminal = RtTerminal::new(backend)?;
        // The terminal cursor stays hidden; render.rs draws the caret itself
        // as a styled cell (plan Context, "Cell model": "Terminal cursor
        // hidden; caret drawn by us (Go parity)").
        terminal.hide_cursor()?;

        Ok(Guard { terminal, events })
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
        // from running (a half-restored terminal is still better than none).
        let backend = self.terminal.backend_mut();
        let _ = write!(
            backend,
            "{}{}{}{}",
            Csi::Keyboard(Keyboard::PopFlags(1)),
            dec_reset(DecPrivateModeCode::BracketedPaste),
            dec_reset(DecPrivateModeCode::ClearAndEnableAlternateScreen),
            dec_set(DecPrivateModeCode::ShowCursor),
        );
        let _ = backend.flush();
        let _ = backend.terminal_mut().enter_cooked_mode();
    }
}
