//! Typed `Command` enum + a stateless resolver table (plan Context,
//! "Keymap"). `resolve` never consults any state — chord *sequences* don't
//! exist in production; the only stateful chord is quit-confirm, and even
//! that lives in `App` (`app::handle_quit_key`), not here. WP5 wires
//! quit-confirm and the resolver end to end; the resolver already covers the
//! full Phase-1 chord table so WP6/7/8 only need to act on the `Command`s it
//! already produces, never touch this file's matching logic again.

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

/// The typed command set (plan Context, "Keymap" table). Movement/editing/
/// clipboard variants are resolved starting WP5 but only acted on starting
/// WP6/7/8 — the plan's "movement commands may no-op until WP6".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    CharLeft,
    CharRight,
    LineUp,
    LineDown,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    PageUp,
    PageDown,
    SelectCharLeft,
    SelectCharRight,
    SelectLineUp,
    SelectLineDown,
    SelectWordLeft,
    SelectWordRight,
    SelectLineStart,
    SelectLineEnd,
    SelectPageUp,
    SelectPageDown,
    SelectAll,
    DeleteLeft,
    DeleteRight,
    Indent,
    Outdent,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Save,
    QuitConfirm,
}

/// Which quit chord produced a `Command::QuitConfirm` — the identity `App`
/// compares to require the SAME chord pressed twice (plan: "press-twice on
/// the SAME chord (ctrl+c ctrl+c or ctrl+alt+d ctrl+alt+d) quits"; pressing
/// the other quit chord does not count as the second press).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitKey {
    CtrlC,
    CtrlAltD,
}

impl QuitKey {
    /// The single source of truth for which `KeyInput`s are quit chords —
    /// `resolve` below routes through this instead of duplicating the
    /// guards, so a `Command::QuitConfirm` and its `QuitKey` identity can
    /// never disagree.
    pub fn from_key(key: KeyInput) -> Option<QuitKey> {
        let m = key.mods;
        match key.code {
            KeyCode::Char('c') if m.ctrl && !m.alt && !m.shift && !m.sup => Some(QuitKey::CtrlC),
            KeyCode::Char('d') if m.ctrl && m.alt && !m.shift && !m.sup => Some(QuitKey::CtrlAltD),
            _ => None,
        }
    }
}

/// The stateless resolver table (plan Context, "Keymap"). `None` means this
/// exact chord isn't bound — the caller's own hardcoded fast paths (Enter,
/// Escape, printable fallthrough — plan: "Hardcoded fast paths outside the
/// resolver") handle everything this function doesn't.
pub fn resolve(key: KeyInput) -> Option<Command> {
    if QuitKey::from_key(key).is_some() {
        return Some(Command::QuitConfirm);
    }

    let m = key.mods;
    match key.code {
        KeyCode::Left => resolve_directional(
            m,
            Command::CharLeft,
            Command::SelectCharLeft,
            Command::WordLeft,
            Command::SelectWordLeft,
        ),
        KeyCode::Right => resolve_directional(
            m,
            Command::CharRight,
            Command::SelectCharRight,
            Command::WordRight,
            Command::SelectWordRight,
        ),
        KeyCode::Up => resolve_plain_or_shift(m, Command::LineUp, Command::SelectLineUp),
        KeyCode::Down => resolve_plain_or_shift(m, Command::LineDown, Command::SelectLineDown),
        KeyCode::Home => resolve_plain_or_shift(m, Command::LineStart, Command::SelectLineStart),
        KeyCode::End => resolve_plain_or_shift(m, Command::LineEnd, Command::SelectLineEnd),
        KeyCode::PageUp => resolve_plain_or_shift(m, Command::PageUp, Command::SelectPageUp),
        KeyCode::PageDown => resolve_plain_or_shift(m, Command::PageDown, Command::SelectPageDown),
        KeyCode::Backspace if m == Mods::NONE => Some(Command::DeleteLeft),
        KeyCode::Delete if m == Mods::NONE => Some(Command::DeleteRight),
        KeyCode::Tab if m == Mods::NONE => Some(Command::Indent),
        KeyCode::Tab if m.shift && !m.alt && !m.ctrl && !m.sup => Some(Command::Outdent),
        KeyCode::BackTab => Some(Command::Outdent),
        KeyCode::Char(c) => resolve_char(c, m),
        _ => None,
    }
}

/// `Left`/`Right`: plain, shift (select), alt (word), shift+alt (select
/// word) — the four-way mirror every other directional-with-word chord
/// shares (plan Keymap table: "alt+left, alt+b / alt+right, alt+f").
fn resolve_directional(
    m: Mods,
    plain: Command,
    select: Command,
    word: Command,
    select_word: Command,
) -> Option<Command> {
    match (m.shift, m.alt, m.ctrl, m.sup) {
        (false, false, false, false) => Some(plain),
        (true, false, false, false) => Some(select),
        (false, true, false, false) => Some(word),
        (true, true, false, false) => Some(select_word),
        _ => None,
    }
}

/// Plain vs. shift (select) only — no alt/word variant (Up/Down/Home/End/
/// PageUp/PageDown).
fn resolve_plain_or_shift(m: Mods, plain: Command, select: Command) -> Option<Command> {
    match (m.shift, m.alt, m.ctrl, m.sup) {
        (false, false, false, false) => Some(plain),
        (true, false, false, false) => Some(select),
        _ => None,
    }
}

/// The `Char(c)`-keyed chords: word motion (`alt+b`/`alt+f`), page motion
/// (`ctrl+u`/`ctrl+d`), select-all, clipboard, undo/redo, save. Quit chords
/// are handled by the `QuitKey::from_key` short-circuit in `resolve` above,
/// not here.
fn resolve_char(c: char, m: Mods) -> Option<Command> {
    match c {
        'b' if m.alt && !m.ctrl && !m.sup => Some(Command::WordLeft),
        'f' if m.alt && !m.ctrl && !m.sup => Some(Command::WordRight),
        'u' if m.ctrl && !m.alt && !m.sup => Some(Command::PageUp),
        'd' if m.ctrl && !m.alt && !m.sup => Some(Command::PageDown),
        'a' if (m.sup || m.ctrl) && !m.alt => Some(Command::SelectAll),
        'c' if m.sup && !m.ctrl && !m.shift => Some(Command::Copy),
        'c' if m.ctrl && m.shift && !m.sup => Some(Command::Copy),
        'x' if m.sup && !m.ctrl => Some(Command::Cut),
        'v' if m.sup && !m.ctrl => Some(Command::Paste),
        'z' if m.sup && !m.shift && !m.ctrl => Some(Command::Undo),
        'z' if m.ctrl && !m.shift && !m.sup => Some(Command::Undo),
        'z' if m.sup && m.shift && !m.ctrl => Some(Command::Redo),
        'y' if m.ctrl && !m.shift && !m.sup => Some(Command::Redo),
        's' if m.sup && !m.ctrl => Some(Command::Save),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: Mods) -> KeyInput {
        KeyInput { code, mods }
    }

    #[test]
    fn plain_arrows_move() {
        assert_eq!(
            resolve(key(KeyCode::Left, Mods::NONE)),
            Some(Command::CharLeft)
        );
        assert_eq!(
            resolve(key(KeyCode::Right, Mods::NONE)),
            Some(Command::CharRight)
        );
        assert_eq!(resolve(key(KeyCode::Up, Mods::NONE)), Some(Command::LineUp));
        assert_eq!(
            resolve(key(KeyCode::Down, Mods::NONE)),
            Some(Command::LineDown)
        );
    }

    #[test]
    fn shift_arrows_select() {
        let shift = Mods {
            shift: true,
            ..Mods::NONE
        };
        assert_eq!(
            resolve(key(KeyCode::Left, shift)),
            Some(Command::SelectCharLeft)
        );
        assert_eq!(
            resolve(key(KeyCode::Up, shift)),
            Some(Command::SelectLineUp)
        );
    }

    #[test]
    fn alt_arrows_and_alt_bf_are_word_motion() {
        let alt = Mods {
            alt: true,
            ..Mods::NONE
        };
        assert_eq!(resolve(key(KeyCode::Left, alt)), Some(Command::WordLeft));
        assert_eq!(resolve(key(KeyCode::Right, alt)), Some(Command::WordRight));
        assert_eq!(
            resolve(key(KeyCode::Char('b'), alt)),
            Some(Command::WordLeft)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('f'), alt)),
            Some(Command::WordRight)
        );
    }

    #[test]
    fn ctrl_c_and_ctrl_alt_d_resolve_to_quit_confirm_with_distinct_identity() {
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
        assert_eq!(resolve(ctrl_c), Some(Command::QuitConfirm));
        assert_eq!(resolve(ctrl_alt_d), Some(Command::QuitConfirm));
        assert_eq!(QuitKey::from_key(ctrl_c), Some(QuitKey::CtrlC));
        assert_eq!(QuitKey::from_key(ctrl_alt_d), Some(QuitKey::CtrlAltD));
        assert_ne!(QuitKey::from_key(ctrl_c), QuitKey::from_key(ctrl_alt_d));
    }

    #[test]
    fn ctrl_shift_c_is_copy_not_quit() {
        let chord = key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                shift: true,
                ..Mods::NONE
            },
        );
        assert_eq!(resolve(chord), Some(Command::Copy));
        assert_eq!(QuitKey::from_key(chord), None);
    }

    #[test]
    fn ctrl_d_is_page_down_not_quit() {
        let chord = key(
            KeyCode::Char('d'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );
        assert_eq!(resolve(chord), Some(Command::PageDown));
    }

    #[test]
    fn tab_and_shift_tab_indent_and_outdent() {
        assert_eq!(
            resolve(key(KeyCode::Tab, Mods::NONE)),
            Some(Command::Indent)
        );
        assert_eq!(
            resolve(key(
                KeyCode::Tab,
                Mods {
                    shift: true,
                    ..Mods::NONE
                }
            )),
            Some(Command::Outdent)
        );
        assert_eq!(
            resolve(key(KeyCode::BackTab, Mods::NONE)),
            Some(Command::Outdent)
        );
    }

    #[test]
    fn super_and_ctrl_a_both_select_all() {
        assert_eq!(
            resolve(key(
                KeyCode::Char('a'),
                Mods {
                    sup: true,
                    ..Mods::NONE
                }
            )),
            Some(Command::SelectAll)
        );
        assert_eq!(
            resolve(key(
                KeyCode::Char('a'),
                Mods {
                    ctrl: true,
                    ..Mods::NONE
                }
            )),
            Some(Command::SelectAll)
        );
    }

    #[test]
    fn unbound_chord_resolves_to_none() {
        assert_eq!(
            resolve(key(
                KeyCode::Char('q'),
                Mods {
                    ctrl: true,
                    alt: true,
                    sup: true,
                    shift: true
                }
            )),
            None
        );
    }
}
