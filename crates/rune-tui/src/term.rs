//! RAII terminal guard: enters raw mode, the alternate screen, Kitty
//! keyboard enhancement flags, bracketed paste, and mouse reporting on
//! construction; `Drop` restores all of it. `Drop` runs
//! during unwind, before `catch_unwind` traps a panic in `rune-cli`'s
//! `main` — the halt-never-panic path this crate requires (panic = "abort"
//! is forbidden precisely so this restore can run).
//!
//! Owns the one `termina::Terminal` for the process: main-thread-only by
//! construction (plan Gotchas: "Cmds must never touch the terminal" —
//! termina's `Terminal` is `io::Write` on `&mut self`, no documented
//! cross-thread write support).
//!
//! Cooked-mode restoration is NOT this module's job: `termina::UnixTerminal`
//! already runs `enter_cooked_mode()` unconditionally from its own `Drop`
//! (the one skip condition, `has_panic_hook && thread::panicking()`, never
//! applies here since this crate never installs a termina panic hook), so
//! `Guard::drop` only needs to reverse the escape sequences it wrote by
//! hand (alt screen, bracketed paste, Kitty flags, cursor visibility).

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
    /// Whether the real terminal behind this `Guard` speaks the Kitty
    /// graphics protocol — `false` at construction, since
    /// `Guard::new` runs before `graphics::detect` can ever measure the
    /// window (`bootstrap`'s own ordering: this field is set right after,
    /// via `set_kitty`, and re-synced on every later `Msg::Resize`). `Drop`
    /// reads it to decide whether tearing down the terminal should emit
    /// `rune_image::encode_delete_all()`: unconditional emission would
    /// violate this crate's Goal that a non-graphics terminal (`TERM=dumb`,
    /// an acceptance run) sees NO escape bytes at all, so `Guard` has to
    /// carry the capability flag rather than assume it.
    kitty: bool,
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

        let mut guard = Guard {
            terminal,
            events,
            kitty: false,
        };
        guard.enter_app_mode()?;
        Ok(guard)
    }

    /// Records whether the terminal speaks the Kitty graphics protocol —
    /// called once right after `graphics::detect` runs in
    /// `bootstrap` (which itself needs a live `Guard` to query the window
    /// through, so this can never be known at `new`'s own construction
    /// time) and again on every `Msg::Resize`, since `app.graphics` is
    /// re-derived there too. Only `Drop` reads it.
    pub fn set_kitty(&mut self, kitty: bool) {
        self.kitty = kitty;
    }

    /// Enables the alternate screen, bracketed paste, Kitty keyboard flags,
    /// and mouse reporting, and hides the terminal cursor (render.rs draws
    /// the caret itself as a styled cell — plan Context, "Cell model":
    /// "Terminal cursor hidden; caret drawn by us"). Only ever
    /// called from `new`, on an already-constructed `Guard` — see its docs
    /// for why that ordering matters.
    ///
    /// Mouse mode 1002 (`ButtonEventMouse`) reports press/
    /// release/drag but never plain hover — mode 1003 (`AnyEventMouse`)
    /// would additionally report every hover, flooding an otherwise idle
    /// event loop with `Moved` events this crate has no gesture for.
    /// `SGRMouse` (mode 1006) is the extended coordinate encoding termina
    /// parses back into `Event::Mouse` without the classic protocol's
    /// 223-column/-row ceiling.
    fn enter_app_mode(&mut self) -> io::Result<()> {
        let backend = self.terminal.backend_mut();
        write!(
            backend,
            "{}{}{}{}{}",
            dec_set(DecPrivateModeCode::ClearAndEnableAlternateScreen),
            dec_set(DecPrivateModeCode::BracketedPaste),
            Csi::Keyboard(Keyboard::PushFlags(keyboard_flags())),
            dec_set(DecPrivateModeCode::ButtonEventMouse),
            dec_set(DecPrivateModeCode::SGRMouse),
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

    /// This terminal's current `(cols, rows, pixel_width, pixel_height)`.
    /// `None` when the underlying dimensions query fails — the caller then
    /// falls back to `rune_image::DEFAULT_CELL_SIZE`.
    pub fn window_size(&mut self) -> Option<(u16, u16, u16, u16)> {
        let dims = self
            .terminal
            .backend_mut()
            .terminal_mut()
            .get_dimensions()
            .ok()?;
        Some((
            dims.cols,
            dims.rows,
            dims.pixel_width.unwrap_or(0),
            dims.pixel_height.unwrap_or(0),
        ))
    }

    pub fn draw(&mut self, f: impl FnOnce(&mut ratatui::Frame)) -> io::Result<()> {
        self.terminal.draw(f)?;
        Ok(())
    }

    /// Resets ratatui's own internal diff buffer, forcing every cell to be
    /// rewritten on the NEXT `draw` — the escape hatch for the one case
    /// ratatui's "only repaint changed cells" diffing gets wrong: a
    /// retransmitted image whose placeholder cells are byte-identical to
    /// the previous frame's (same id, same diacritics) even though the
    /// PIXELS the terminal shows at those cells just changed underneath
    /// them.
    pub fn force_redraw(&mut self) {
        if let Ok(size) = self.terminal.size() {
            let _ = self.terminal.resize(ratatui::layout::Rect::from(size));
        }
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

/// The exact bytes `Guard::drop` writes to restore the terminal — a pure
/// function so the Kitty-gating decision is unit-testable
/// without a live `PlatformTerminal` (which `Guard` itself can't be
/// constructed without in a headless test environment). `kitty` decides
/// only the LEADING `encode_delete_all()` escape: Kitty images stay
/// resident in the terminal until explicitly deleted, so quitting must
/// clear them all, but ONLY on a terminal that actually speaks the
/// protocol — unconditional emission would violate this crate's Goal
/// that a non-graphics terminal sees no escape bytes at all. The
/// mode-restoring escapes after it are unconditional, exactly as
/// before this package.
fn teardown_bytes(kitty: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    if kitty {
        bytes.extend_from_slice(rune_image::encode_delete_all().as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "{}{}{}{}{}{}",
            Csi::Keyboard(Keyboard::PopFlags(KITTY_FLAGS_PUSHED)),
            dec_reset(DecPrivateModeCode::SGRMouse),
            dec_reset(DecPrivateModeCode::ButtonEventMouse),
            dec_reset(DecPrivateModeCode::BracketedPaste),
            dec_reset(DecPrivateModeCode::ClearAndEnableAlternateScreen),
            dec_set(DecPrivateModeCode::ShowCursor),
        )
        .as_bytes(),
    );
    bytes
}

impl Drop for Guard {
    fn drop(&mut self) {
        // Halt, never panic — every step here is best-effort. A
        // failure restoring one escape sequence must not prevent the rest
        // from running (a half-restored terminal is still better than
        // none). `termina::UnixTerminal`'s own `Drop` restores cooked mode
        // once `self.terminal` (and the `PlatformTerminal` it owns) is
        // dropped after this — see module docs.
        let backend = self.terminal.backend_mut();
        let _ = backend.write_all(&teardown_bytes(self.kitty));
        let _ = backend.flush();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Asserted against the pure byte-builder `Guard::drop` itself calls,
    /// since `Guard` cannot be constructed at all without a live terminal.
    #[test]
    fn teardown_emits_delete_all_only_when_kitty_is_available() {
        let with_kitty = teardown_bytes(true);
        assert!(with_kitty.starts_with(rune_image::encode_delete_all().as_bytes()));

        let without_kitty = teardown_bytes(false);
        assert!(
            !without_kitty
                .windows(b"\x1b_G".len())
                .any(|w| w == b"\x1b_G"),
            "a non-Kitty terminal must see no image escape bytes at all"
        );
    }
}
