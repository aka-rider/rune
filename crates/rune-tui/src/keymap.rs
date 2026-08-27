pub mod editor_bindings;
pub mod index;
mod keyinput;
pub mod vim;

pub use crate::binding::{Binding, KeyOutcome, KeyPattern, resolve_in};
pub use crate::global::{GLOBAL_BINDINGS, GlobalCommand};
pub use keyinput::{KeyCode, KeyInput, Mods, from_termina};
pub use vim::{BindingSet, VIM_BINDINGS, VimCommand};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
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
    MatchBracket,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extend {
    No,
    Yes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Motion(Motion, Extend),
    SelectAll,
    DeleteLeft,
    DeleteRight,
    DeleteWordLeft,
    DeleteWordRight,
    DeleteLine,
    Indent,
    Outdent,
    MoveLineUp,
    MoveLineDown,
    CloneLineUp,
    CloneLineDown,
    AddCursorAbove,
    AddCursorBelow,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    Save,
    QuitConfirm,
    // Moves the viewport only; the cursor moves only if the scroll pushes
    // it off screen.
    ScrollLineUp,
    ScrollLineDown,
    // Viewport-only half-page scroll, distinct from `PageUp`/`PageDown`
    // above, which move the cursor a full page.
    ScrollHalfPageUp,
    ScrollHalfPageDown,
    // Viewport-only: re-centres on the cursor's row without moving the
    // cursor.
    CentreCursor,
    // Viewport-only: scrolls the cursor's row to the top without moving
    // the cursor.
    CursorToTop,
    // Viewport-only: scrolls the cursor's row to the bottom without
    // moving the cursor.
    CursorToBottom,
    FollowLink,
    Reload,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitKey {
    CtrlC,
    CtrlD,
}

impl QuitKey {
    pub fn from_key(key: KeyInput) -> Option<QuitKey> {
        let m = key.mods;
        match key.code {
            KeyCode::Char('c') if m.ctrl && !m.alt && !m.shift && !m.sup => Some(QuitKey::CtrlC),
            KeyCode::Char('d') if m.ctrl && !m.alt && !m.shift && !m.sup => Some(QuitKey::CtrlD),
            _ => None,
        }
    }
}

pub fn resolve(key: KeyInput) -> Option<Command> {
    if QuitKey::from_key(key).is_some() {
        return Some(Command::QuitConfirm);
    }
    resolve_in(editor_bindings::EDITOR_BINDINGS, key)
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
            Some(Command::Motion(Motion::CharLeft, Extend::No))
        );
        assert_eq!(
            resolve(key(KeyCode::Right, Mods::NONE)),
            Some(Command::Motion(Motion::CharRight, Extend::No))
        );
        assert_eq!(
            resolve(key(KeyCode::Up, Mods::NONE)),
            Some(Command::Motion(Motion::LineUp, Extend::No))
        );
        assert_eq!(
            resolve(key(KeyCode::Down, Mods::NONE)),
            Some(Command::Motion(Motion::LineDown, Extend::No))
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
            Some(Command::Motion(Motion::CharLeft, Extend::Yes))
        );
        assert_eq!(
            resolve(key(KeyCode::Up, shift)),
            Some(Command::Motion(Motion::LineUp, Extend::Yes))
        );
    }

    #[test]
    fn alt_arrows_are_word_motion_and_alt_letters_stay_unbound() {
        let alt = Mods {
            alt: true,
            ..Mods::NONE
        };
        assert_eq!(
            resolve(key(KeyCode::Left, alt)),
            Some(Command::Motion(Motion::WordLeft, Extend::No))
        );
        assert_eq!(
            resolve(key(KeyCode::Right, alt)),
            Some(Command::Motion(Motion::WordRight, Extend::No))
        );
        assert_eq!(
            resolve(key(KeyCode::Char('b'), alt)),
            None,
            "macOS composes ⌥B into ∫ before any modifier reaches us"
        );
        assert_eq!(
            resolve(key(KeyCode::Char('f'), alt)),
            None,
            "macOS composes ⌥F into ƒ before any modifier reaches us"
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
        assert_eq!(
            resolve(chord),
            Some(Command::Motion(Motion::PageUp, Extend::No))
        );
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
    }

    #[test]
    fn shift_tab_arrives_from_termina_as_backtab_and_resolves_to_outdent() {
        use termina::event::{
            KeyCode as TK, KeyEvent, KeyEventKind, KeyEventState, Modifiers as TM,
        };

        let shift = Mods {
            shift: true,
            ..Mods::NONE
        };

        for modifiers in [TM::SHIFT, TM::NONE] {
            let input = from_termina(KeyEvent {
                code: TK::BackTab,
                modifiers,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            });
            assert_eq!(input, Some(key(KeyCode::Tab, shift)));
            assert_eq!(input.and_then(resolve), Some(Command::Outdent));
        }
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

    #[test]
    fn save_requires_exact_mods_and_shifted_variants_resolve_to_none() {
        let sup_shift = key(
            KeyCode::Char('s'),
            Mods {
                sup: true,
                shift: true,
                ..Mods::NONE
            },
        );
        let sup_alt = key(
            KeyCode::Char('s'),
            Mods {
                sup: true,
                alt: true,
                ..Mods::NONE
            },
        );
        assert_eq!(resolve(sup_shift), None, "⌘⇧S must not resolve to a save");
        assert_eq!(resolve(sup_alt), None, "⌘⌥S must not resolve to a save");
    }

    #[test]
    fn shift_alt_arrows_select_word_not_move() {
        let shift_alt = Mods {
            shift: true,
            alt: true,
            ..Mods::NONE
        };
        assert_eq!(
            resolve(key(KeyCode::Left, shift_alt)),
            Some(Command::Motion(Motion::WordLeft, Extend::Yes))
        );
        assert_eq!(
            resolve(key(KeyCode::Right, shift_alt)),
            Some(Command::Motion(Motion::WordRight, Extend::Yes))
        );
    }

    // The converse check: every chord `resolve` accepts must have a
    // matching `EDITOR_BINDINGS` row, or it could resolve live yet stay
    // invisible to the generated Help doc and the startup collision index,
    // which both only ever read that table. Sweeps every printable ASCII
    // `Char` against all 16 `Mods` combinations (~1500 cases).
    #[test]
    fn every_resolving_char_chord_has_an_editor_bindings_row() {
        let mod_combos: Vec<Mods> = (0u8..16)
            .map(|bits| Mods {
                shift: bits & 0b0001 != 0,
                alt: bits & 0b0010 != 0,
                ctrl: bits & 0b0100 != 0,
                sup: bits & 0b1000 != 0,
            })
            .collect();

        let mut checked = 0usize;
        for c in ' '..='~' {
            for &m in &mod_combos {
                checked += 1;
                let k = key(KeyCode::Char(c), m);
                if QuitKey::from_key(k).is_some() {
                    continue;
                }
                let Some(cmd) = resolve(k) else { continue };
                assert_eq!(
                    resolve_in(editor_bindings::EDITOR_BINDINGS, k),
                    Some(cmd),
                    "{c:?} with {m:?} resolves to {cmd:?} live but has no EDITOR_BINDINGS row"
                );
            }
        }
        assert!(
            checked >= 1500,
            "sweep should cover roughly 1500 cases, covered {checked}"
        );
    }
}
