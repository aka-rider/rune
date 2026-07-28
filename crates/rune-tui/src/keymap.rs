//! Typed `Command` enum + a stateless resolver table (plan Context,
//! "Keymap"). `resolve` never consults any state and stays hand-written —
//! it's still the LIVE dispatch path `app::handle_editor_key` calls; WP6
//! adds `editor_bindings::EDITOR_BINDINGS`, a data table mirroring the same
//! chords, purely so the generated Help doc (`help.rs`) and the startup
//! collision index (`index.rs`) have something to read without a hand-
//! maintained second copy — it does not replace `resolve` as the thing that
//! actually executes a keystroke. The held-space leader (`global::
//! LEADER_BINDINGS`) is a separate, already-live stateful mechanism (see
//! `keystate.rs`/`app::handle_key`'s stage 1.5); `index::KeymapState` below
//! is a second, general-purpose sequence tracker for a future binding-table-
//! driven chord, not a replacement for it.

// The generic binding machinery now lives in `crate::binding` and the
// global chord table in `crate::global` (§1.6: this file was over the
// 500-line budget). Re-exported here so every existing `keymap::` import
// path keeps working.
//
// `index`/`editor_bindings`/`vim` are submodules of THIS file (plan WP6):
// Rust lets a `foo.rs` module have its submodules live under `foo/` even
// though `foo.rs` itself is not `foo/mod.rs` — so `keymap.rs` stays the
// single top-level file the rest of the crate already imports from, while
// its new WP6 machinery gets its own files instead of growing this one
// past the §1.6 budget again.
pub mod editor_bindings;
pub mod index;
pub mod vim;

pub use crate::binding::{Binding, KeyOutcome, KeyPattern, resolve_in};
pub use crate::global::{GLOBAL_BINDINGS, GlobalCommand};
pub use index::{KeymapState, NextKeyFn, Resolution};
pub use vim::{BindingSet, VIM_BINDINGS, VimCommand};

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
    /// Plan WP9.S2 — `⌥⌫`/`⌥⌦` (Option+Backspace/Delete).
    DeleteWordLeft,
    DeleteWordRight,
    /// Plan WP9.S2 — `⌘⇧K`, unbound in the Go original (see
    /// `editor_bindings.rs`'s module doc).
    DeleteLine,
    Indent,
    Outdent,
    /// Plan WP9.S2 — `⌥↑`/`⌥↓`, matching Go's own `keymap.go` bindings.
    MoveLineUp,
    MoveLineDown,
    /// Plan WP9.S2 — `⌥⇧↑`/`⌥⇧↓`, unbound in the Go original.
    CloneLineUp,
    CloneLineDown,
    /// Plan WP9.S3 — `⌥⌘↑`/`⌥⌘↓`, matching Go's own `keymap.go` bindings.
    AddCursorAbove,
    AddCursorBelow,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Save,
    QuitConfirm,
    /// Viewport-only scroll (plan WP7.S2): vim `ctrl+e`/Helix
    /// `scroll_line_up`/`down` — moves `Viewport::scroll_row` by one row,
    /// never the cursor (unless the scroll pushes it off screen; see
    /// `Viewport::reconcile`'s docs).
    ScrollLineUp,
    ScrollLineDown,
    /// vim/Helix `ctrl+u`/`ctrl+d`-style half-page scroll — viewport-only,
    /// `commands::scroll(..., sync_cursor: false)` (Helix). Distinct from
    /// `PageUp`/`PageDown` above, which move the CURSOR a full page.
    ScrollHalfPageUp,
    ScrollHalfPageDown,
    /// vim/Helix `zz`: re-centres the viewport on the cursor's row.
    CentreCursor,
    /// vim/Helix `zt`: scrolls the cursor's row to the top of the viewport.
    CursorToTop,
    /// vim/Helix `zb`: scrolls the cursor's row to the bottom of the
    /// viewport.
    CursorToBottom,
}

/// Which quit chord produced a `Command::QuitConfirm` — the identity `App`
/// compares to require the SAME chord pressed twice: the two quit chords
/// are `ctrl+c ctrl+c` and `ctrl+d ctrl+d`; pressing the other quit chord
/// does not count as the second press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitKey {
    CtrlC,
    CtrlD,
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
            KeyCode::Char('d') if m.ctrl && !m.alt && !m.shift && !m.sup => Some(QuitKey::CtrlD),
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
    const CTRL_ONLY: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    };
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
        // `ctrl+Up`/`ctrl+Down` (plan WP7.S2/S7): viewport-only line scroll
        // — VS Code's own default binding for "Scroll Line Up"/"Scroll Line
        // Down". Free: `resolve_vertical` never matches a combination with
        // CTRL set.
        KeyCode::Up if m == CTRL_ONLY => Some(Command::ScrollLineUp),
        KeyCode::Up => resolve_vertical(
            m,
            Command::LineUp,
            Command::SelectLineUp,
            Command::MoveLineUp,
            Command::CloneLineUp,
            Command::AddCursorAbove,
        ),
        KeyCode::Down if m == CTRL_ONLY => Some(Command::ScrollLineDown),
        KeyCode::Down => resolve_vertical(
            m,
            Command::LineDown,
            Command::SelectLineDown,
            Command::MoveLineDown,
            Command::CloneLineDown,
            Command::AddCursorBelow,
        ),
        // `ctrl+Home`/`ctrl+End`: scroll the cursor's row to the top/bottom
        // of the viewport (vim/Helix `zt`/`zb`) — free of `Home`/`End`'s
        // own NONE/SHIFT-only arms.
        KeyCode::Home if m == CTRL_ONLY => Some(Command::CursorToTop),
        KeyCode::Home => resolve_plain_or_shift(m, Command::LineStart, Command::SelectLineStart),
        KeyCode::End if m == CTRL_ONLY => Some(Command::CursorToBottom),
        KeyCode::End => resolve_plain_or_shift(m, Command::LineEnd, Command::SelectLineEnd),
        // `ctrl+PageUp`/`ctrl+PageDown`: half-page viewport-only scroll
        // (vim/Helix `ctrl+u`/`ctrl+d`) — distinct from the plain `ctrl+u`
        // `Char` chord below, which stays bound to the full-page CURSOR
        // motion `Command::PageUp`.
        KeyCode::PageUp if m == CTRL_ONLY => Some(Command::ScrollHalfPageUp),
        KeyCode::PageUp => resolve_plain_or_shift(m, Command::PageUp, Command::SelectPageUp),
        KeyCode::PageDown if m == CTRL_ONLY => Some(Command::ScrollHalfPageDown),
        KeyCode::PageDown => resolve_plain_or_shift(m, Command::PageDown, Command::SelectPageDown),
        KeyCode::Backspace if m == Mods::NONE => Some(Command::DeleteLeft),
        KeyCode::Backspace if m.alt && !m.ctrl && !m.sup && !m.shift => {
            Some(Command::DeleteWordLeft)
        }
        KeyCode::Delete if m == Mods::NONE => Some(Command::DeleteRight),
        KeyCode::Delete if m.alt && !m.ctrl && !m.sup && !m.shift => Some(Command::DeleteWordRight),
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

/// Plain vs. shift (select) only — no alt/word variant (Home/End/PageUp/
/// PageDown).
fn resolve_plain_or_shift(m: Mods, plain: Command, select: Command) -> Option<Command> {
    match (m.shift, m.alt, m.ctrl, m.sup) {
        (false, false, false, false) => Some(plain),
        (true, false, false, false) => Some(select),
        _ => None,
    }
}

/// `Up`/`Down`: plain, shift (select), alt (move-line), shift+alt
/// (clone-line), alt+super (add-cursor) — plan WP9.S2/S3's five-way
/// mirror of `resolve_directional`'s word variant, using vertical-motion
/// commands instead of word motion.
fn resolve_vertical(
    m: Mods,
    plain: Command,
    select: Command,
    alt: Command,
    shift_alt: Command,
    alt_sup: Command,
) -> Option<Command> {
    match (m.shift, m.alt, m.ctrl, m.sup) {
        (false, false, false, false) => Some(plain),
        (true, false, false, false) => Some(select),
        (false, true, false, false) => Some(alt),
        (true, true, false, false) => Some(shift_alt),
        (false, true, false, true) => Some(alt_sup),
        _ => None,
    }
}

/// The `Char(c)`-keyed chords: word motion (`alt+b`/`alt+f`), page motion
/// (`ctrl+u`), select-all, clipboard, undo/redo, save, delete-line
/// (`sup+shift+k`, plan WP9.S2). Quit chords are handled by the
/// `QuitKey::from_key` short-circuit in `resolve` above, not here.
fn resolve_char(c: char, m: Mods) -> Option<Command> {
    match c {
        'b' if m.alt && !m.ctrl && !m.sup => Some(Command::WordLeft),
        'f' if m.alt && !m.ctrl && !m.sup => Some(Command::WordRight),
        'u' if m.ctrl && !m.alt && !m.sup => Some(Command::PageUp),
        // vim/Helix `zz` (plan WP7.S2/S7) — re-centre the viewport on the
        // cursor's row; Emacs's own `C-l` "recenter" precedent for the
        // chord itself.
        'l' if m.ctrl && !m.alt && !m.shift && !m.sup => Some(Command::CentreCursor),
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
        'k' if m.sup && m.shift && !m.ctrl && !m.alt => Some(Command::DeleteLine),
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
    fn ctrl_c_and_ctrl_d_resolve_to_quit_confirm_with_distinct_identity() {
        let ctrl_c = key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );
        let ctrl_d = key(
            KeyCode::Char('d'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );
        assert_eq!(resolve(ctrl_c), Some(Command::QuitConfirm));
        assert_eq!(resolve(ctrl_d), Some(Command::QuitConfirm));
        assert_eq!(QuitKey::from_key(ctrl_c), Some(QuitKey::CtrlC));
        assert_eq!(QuitKey::from_key(ctrl_d), Some(QuitKey::CtrlD));
        assert_ne!(QuitKey::from_key(ctrl_c), QuitKey::from_key(ctrl_d));
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
    fn ctrl_d_is_quit_not_page_down() {
        let chord = key(
            KeyCode::Char('d'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );
        assert_eq!(resolve(chord), Some(Command::QuitConfirm));
    }

    #[test]
    fn ctrl_u_is_still_page_up() {
        let chord = key(
            KeyCode::Char('u'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );
        assert_eq!(resolve(chord), Some(Command::PageUp));
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

    // The generic machinery (`resolve_in`/`KeyPattern`) now lives in
    // `binding.rs` and the global table in `global.rs`; their coverage is
    // in `tests/keymap_global.rs`.
}
