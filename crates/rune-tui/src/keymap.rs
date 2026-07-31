//! Typed `Command` enum + a stateless resolver (plan Context, "Keymap").
//! `resolve` never consults any state; it IS the LIVE dispatch path
//! `app::handle_editor_key` calls (plan WP10.S3), and it is now a thin
//! wrapper around `resolve_in(editor_bindings::EDITOR_BINDINGS, key)` —
//! the data table is the one source of truth, not a mirror kept in sync by
//! hand. `resolve_in`'s whole-`Mods` matching (`KeyPattern::matches`) is
//! load-bearing: a hand-written `match` guard can check a subset of a
//! chord's modifiers and let something else through by accident (the
//! defect this replaced — `CODE-REVIEW.md`'s rune-tui B finding 3: a loose
//! `'s' if m.sup && !m.ctrl` arm let `⌘⇧S` perform a real save); a table
//! lookup cannot. The held-space leader (`global::LEADER_BINDINGS`) is a
//! separate, already-live stateful mechanism (see `keystate.rs`/
//! `app::handle_key`'s stage 1.5); `index::KeymapState` below is a second,
//! general-purpose sequence tracker for a future binding-table-driven
//! chord, not a replacement for it.

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
mod keyinput;
pub mod vim;

pub use crate::binding::{Binding, KeyOutcome, KeyPattern, resolve_in};
pub use crate::global::{GLOBAL_BINDINGS, GlobalCommand};
pub use index::{KeymapState, NextKeyFn, Resolution};
pub use keyinput::{KeyCode, KeyInput, Mods, from_termina};
pub use vim::{BindingSet, VIM_BINDINGS, VimCommand};

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
    /// Follows the link under the cursor (plan WP5.S7) — ⌘Enter or ^Enter.
    /// Deliberately distinct from the hardcoded plain-Enter newline fast
    /// path (`app::handle_editor_key`, `Mods::NONE` only), so the two can
    /// never collide.
    FollowLink,
    /// Re-reads an image document through the `Vfs`, re-decodes it, and
    /// retransmits under the same deterministic id (plan WP6.S1) — bound
    /// to `⌘R`, gated by the `image` `when` atom (plan WP6.S2) so it only
    /// ever does anything on an image document; `graphics::reload_image`
    /// is itself a no-op on any other document, so the gate is a UX
    /// signal (footer/help visibility) rather than the only thing standing
    /// between this chord and a real editor document.
    Reload,
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

/// The stateless resolver (plan Context, "Keymap"; plan WP10.S3). `None`
/// means this exact chord isn't bound — the caller's own hardcoded fast
/// paths (Enter, Escape, printable fallthrough — plan: "Hardcoded fast
/// paths outside the resolver") handle everything this function doesn't.
/// The quit chords are the one exception kept outside the table: they are
/// identity-bearing (`QuitKey`, threaded through `App::pending_quit`) in a
/// way a plain `Command` isn't, so `QuitKey::from_key` stays the single
/// source of truth for them, same as before.
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

    /// Regression for `CODE-REVIEW.md` rune-tui B finding 3: a loose
    /// `resolve_char` arm (`'s' if m.sup && !m.ctrl`, never checking
    /// `shift`/`alt`) let `⌘⇧S` and `⌘⌥S` perform a real in-place save via
    /// `Command::Save`. `EDITOR_BINDINGS` has a row for the EXACT `sup`-only
    /// chord (see its own doc comment for why), but `resolve_in`'s
    /// whole-`Mods` matching means no chord holding `shift` or `alt`
    /// alongside `sup+s` can match that row or any other.
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

    /// Regression for `CODE-REVIEW.md` rune-tui B finding 3's other half:
    /// `⌥⇧B`/`⌥⇧F` must SELECT word-left/right, not collapse a selection
    /// by silently falling back to plain word motion (the old
    /// `resolve_char` guard didn't check `shift` on the ALT arm either).
    #[test]
    fn shift_alt_bf_selects_word_not_moves() {
        let shift_alt = Mods {
            shift: true,
            alt: true,
            ..Mods::NONE
        };
        assert_eq!(
            resolve(key(KeyCode::Char('b'), shift_alt)),
            Some(Command::SelectWordLeft)
        );
        assert_eq!(
            resolve(key(KeyCode::Char('f'), shift_alt)),
            Some(Command::SelectWordRight)
        );
    }

    /// The converse of `editor_bindings`'s own
    /// `every_row_resolves_through_the_live_dispatch_path`: every chord
    /// `resolve` DOES accept must have a matching `EDITOR_BINDINGS` row —
    /// otherwise a chord could resolve live yet vanish from the generated
    /// Help doc and the startup collision index, both of which only ever
    /// read the table (`CODE-REVIEW.md` rune-tui B finding 4). Sweeps
    /// every printable ASCII `Char` against all 16 `Mods` combinations
    /// (~1500 cases) — cheap at this table's size, and it is what would
    /// have caught finding 3 directly, since a resolving-but-tableless
    /// chord is exactly what a loose `resolve_char` arm produced.
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
                    continue; // Quit chords deliberately have no table row.
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

    // The generic machinery (`resolve_in`/`KeyPattern`) now lives in
    // `binding.rs` and the global table in `global.rs`; their coverage is
    // in `tests/keymap_global.rs`.
}
