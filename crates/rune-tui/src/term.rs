use std::io::{self, Write};

use ratatui::Terminal as RtTerminal;
use ratatui::backend::TerminaBackend;
use termina::Terminal as _;
use termina::escape::csi::{
    Csi, DecPrivateMode, DecPrivateModeCode, Keyboard, KittyKeyboardFlags, Mode,
};
use termina::{EventReader, PlatformTerminal};

// A terminal without Kitty support ignores this CSI outright.
pub(crate) fn keyboard_flags() -> KittyKeyboardFlags {
    KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES | KittyKeyboardFlags::REPORT_ALTERNATE_KEYS
}

pub(crate) fn sup_chords_reliable(flags: Option<KittyKeyboardFlags>) -> bool {
    flags.is_none_or(|flags| flags.contains(keyboard_flags()))
}

const KITTY_FLAGS_PUSHED: u8 = 1;

fn dec_set(code: DecPrivateModeCode) -> Csi {
    Csi::Mode(Mode::SetDecPrivateMode(DecPrivateMode::Code(code)))
}

fn dec_reset(code: DecPrivateModeCode) -> Csi {
    Csi::Mode(Mode::ResetDecPrivateMode(DecPrivateMode::Code(code)))
}

pub struct Guard {
    terminal: RtTerminal<TerminaBackend<PlatformTerminal>>,
    events: EventReader,
    kitty: bool,
}

impl Guard {
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

    pub fn set_kitty(&mut self, kitty: bool) {
        self.kitty = kitty;
    }

    // Mouse mode 1002 reports press/release/drag but never plain hover;
    // mode 1003 would additionally flood an idle event loop with `Moved`
    // events this crate has no gesture for. `SGRMouse` (mode 1006) extends
    // mouse coordinates past the classic protocol's 223-column ceiling.
    fn enter_app_mode(&mut self) -> io::Result<()> {
        let backend = self.terminal.backend_mut();
        write!(backend, "{}", app_mode_bytes())?;
        backend.flush()?;
        self.terminal.hide_cursor()
    }

    pub fn event_reader(&self) -> EventReader {
        self.events.clone()
    }

    pub fn size(&self) -> io::Result<(u16, u16)> {
        let size = self.terminal.size()?;
        Ok((size.width, size.height))
    }

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

    // Forces every cell to be rewritten on the next draw: the escape hatch
    // for the one case ratatui's "only repaint changed cells" diffing gets
    // wrong — a retransmitted image whose placeholder cells are
    // byte-identical to the previous frame's even though the terminal's
    // pixels underneath just changed.
    pub fn force_redraw(&mut self) {
        if let Ok(size) = self.terminal.size() {
            let _ = self.terminal.resize(ratatui::layout::Rect::from(size));
        }
    }

    pub fn write_raw(&mut self, bytes: &[u8]) -> io::Result<()> {
        let backend = self.terminal.backend_mut();
        backend.write_all(bytes)?;
        backend.flush()
    }
}

fn app_mode_bytes() -> String {
    format!(
        "{}{}{}{}{}{}",
        dec_set(DecPrivateModeCode::ClearAndEnableAlternateScreen),
        dec_set(DecPrivateModeCode::BracketedPaste),
        Csi::Keyboard(Keyboard::PushFlags(keyboard_flags())),
        Csi::Keyboard(Keyboard::QueryFlags),
        dec_set(DecPrivateModeCode::ButtonEventMouse),
        dec_set(DecPrivateModeCode::SGRMouse),
    )
}

// Kitty images stay resident in the terminal until explicitly deleted, so
// quitting must clear them all — but only on a terminal that actually
// speaks the protocol.
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
        // Runs during unwind, before rune-cli's catch_unwind traps a panic;
        // panic = "abort" is forbidden precisely so this restore still runs.
        // termina's own `UnixTerminal` Drop restores cooked mode once
        // `self.terminal` drops after this; only the escapes written by
        // hand need reversing here.
        let backend = self.terminal.backend_mut();
        let _ = backend.write_all(&teardown_bytes(self.kitty));
        let _ = backend.flush();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

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

    #[test]
    fn app_mode_queries_back_the_kitty_flags_it_just_pushed() {
        let bytes = app_mode_bytes();
        assert!(bytes.contains(&Csi::Keyboard(Keyboard::PushFlags(keyboard_flags())).to_string()));
        assert!(bytes.contains(&Csi::Keyboard(Keyboard::QueryFlags).to_string()));
    }

    #[test]
    fn an_unanswered_probe_reads_as_reliable_not_broken() {
        assert!(sup_chords_reliable(None));
    }

    #[test]
    fn a_reply_confirming_both_requested_bits_reads_as_reliable() {
        assert!(sup_chords_reliable(Some(keyboard_flags())));
    }

    #[test]
    fn a_reply_missing_a_requested_bit_reads_as_unreliable() {
        assert!(!sup_chords_reliable(Some(
            KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
        )));
        assert!(!sup_chords_reliable(Some(KittyKeyboardFlags::NONE)));
    }
}
