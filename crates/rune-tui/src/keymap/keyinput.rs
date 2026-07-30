//! The platform key-identity layer: `KeyCode`/`Mods`/`KeyInput` plus the one
//! bridge from termina's own event type (`from_termina`). Split out of
//! `keymap.rs` to bring that file under the §1.6 500-line budget, mirroring
//! the `binding.rs`/`global.rs` extraction already used for the generic
//! table machinery and the global chord table; `keymap` re-exports every
//! item here so no import path downstream changed.

/// A platform- and library-independent key identity — decoupled from
/// termina's `KeyCode` so the resolver table below (and its tests) don't
/// depend on termina at all. `from_termina` is the only place that bridges
/// the two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Backspace,
    Tab,
    BackTab,
    Escape,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    /// The F1 function key — bound to `GlobalCommand::Help` (WP2/WP7)
    /// below. The only `Function(u8)` termina reports this crate binds; no
    /// other function key is meaningful here yet.
    F1,
}

/// Modifier keys held during a key event. Field names avoid `super` (a
/// reserved path keyword) and spell out `sup` for the Command/Super key —
/// Command on macOS, the platform this app exclusively targets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub sup: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: false,
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInput {
    pub code: KeyCode,
    pub mods: Mods,
}

/// Translate a termina key event to a `KeyInput`, or `None` for a key this
/// app doesn't bind (function keys, media keys, ...) or a Release event —
/// commands act on `Press`/`Repeat` only (termina docs: "Code that handles
/// shortcuts should usually check `kind == KeyEventKind::Press`"; `Repeat`
/// is treated the same as `Press` so a held arrow key keeps moving).
pub fn from_termina(event: termina::event::KeyEvent) -> Option<KeyInput> {
    use termina::event::{KeyCode as TK, KeyEventKind, Modifiers as TM};

    if !matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }

    let code = match event.code {
        TK::Char(c) => KeyCode::Char(c),
        TK::Enter => KeyCode::Enter,
        TK::Backspace => KeyCode::Backspace,
        TK::Tab => KeyCode::Tab,
        TK::BackTab => KeyCode::BackTab,
        TK::Escape => KeyCode::Escape,
        TK::Left => KeyCode::Left,
        TK::Right => KeyCode::Right,
        TK::Up => KeyCode::Up,
        TK::Down => KeyCode::Down,
        TK::Home => KeyCode::Home,
        TK::End => KeyCode::End,
        TK::PageUp => KeyCode::PageUp,
        TK::PageDown => KeyCode::PageDown,
        TK::Delete => KeyCode::Delete,
        TK::Function(1) => KeyCode::F1,
        _ => return None,
    };

    let m = event.modifiers;
    let mods = Mods {
        shift: m.contains(TM::SHIFT),
        alt: m.contains(TM::ALT),
        ctrl: m.contains(TM::CONTROL),
        sup: m.contains(TM::SUPER),
    };
    Some(KeyInput { code, mods })
}
