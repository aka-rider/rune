//! The global chord table — the bindings resolved before any pane's own
//! keymap. Split out of `keymap.rs` to bring that file under the §1.6
//! 500-line budget; `keymap` re-exports `GlobalCommand`/`GLOBAL_BINDINGS`
//! so no import path downstream changed.

use crate::binding::{Binding, KeyPattern};
use crate::keymap::{KeyCode, Mods, QuitKey};

/// The global chord table's command set (decision 7: `Pane` focus
/// discriminant + these chrome-level actions). Resolved BEFORE any pane's
/// own keymap, so every variant fires regardless of focus — including the
/// quit chords and Save, which must keep working while the Explorer/Tabs
/// stub panes own it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalCommand {
    /// Always exposes and focuses the Explorer — never hides it, mirroring
    /// the Go reference and matching `FocusTabs`'s own show-plus-focus
    /// contract below.
    FocusExplorer,
    FocusEditor,
    /// Focuses the Open Tabs pane. Explorer/Tabs are separate panes, so,
    /// unlike Go's single shared explorer chord, this needs its own binding.
    /// Shows the left column too, pairing show with focus exactly like
    /// `FocusExplorer` does, so the tab list is actually visible the moment
    /// it's focused.
    FocusTabs,
    /// Hides the left column, undoing whichever of `FocusExplorer`/
    /// `FocusTabs` last showed it. The collapse counterpart to those two:
    /// where they always show and focus, this always hides and, if the
    /// column currently owns focus, hands it back to the Editor.
    CollapseLeft,
    /// Focuses the title field so the active document can be renamed (`^r`
    /// — free across `GLOBAL_BINDINGS`, `resolve_char`, `TABS_BINDINGS` and
    /// `EXPLORER_BINDINGS`). Global rather than editor-scoped so a rename is
    /// reachable from any pane, exactly like Save.
    FocusTitle,
    Save,
    Help,
    QuitChord(QuitKey),
    /// Closes the active document from any pane focus — routed through the
    /// one close chokepoint (`workspace::request_close`) so a dirty document
    /// still arms its Guard regardless of which pane the chord was pressed
    /// from, exactly like `Save` already does for materialize.
    CloseFile,
    /// Switches to the tab at this zero-based position; out of range is a
    /// silent no-op. The digit is already resolved into the payload by
    /// `GLOBAL_BINDINGS` below, so the digit-to-index mapping lives in
    /// exactly one place rather than being re-derived at the call site.
    TabSwitch(usize),
    /// Toggles the active document between `ReadOnly::No` and `ReadOnly::
    /// Reading` (plan WP5) — the same chord both enters and leaves reading
    /// view. Refused with a status message on `ReadOnly::Always`, which has
    /// no editable form to return to.
    ToggleReadOnly,
}

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};
const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};

/// The five focus/chrome commands each get a ⌘ and a `^` chord (the leader
/// they used to share is gone — a terminal cannot report the spacebar's
/// physical state in-band, so a prefix chord can never be told apart from
/// plain text; see the module removed at `keystate.rs`). One form of each
/// pair is marked `alias: true` so the footer's hint row names the command
/// once while the Help doc still lists both; which form stays canonical
/// keeps the shorter `^` glyph in the footer. Ghostty intercepts some ⌘
/// chords before the app ever sees them (⌘T, ⌘K in particular), so `^` is
/// the form guaranteed to arrive — both are bound regardless, since a
/// different terminal may pass the ⌘ form through.
///
/// `Save` and the two quit chords are the SAME combos `resolve`/
/// `QuitKey::from_key` already bind — moving their resolution to the global
/// pipeline stage changes only WHEN they're seen (before, not after, a
/// pane's own keymap), not which chord activates them. `KeyPattern`'s
/// exact-modifier match narrows `resolve_char`'s `'s' if m.sup && !m.ctrl`
/// guard (which also tolerated shift/alt held) to the one precise combo
/// below — the loosely-matched variants were never a documented,
/// intentional binding.
///
/// A ctrl chord that duplicates its ⌘ counterpart (or, for `^d`, another
/// quit chord) is marked `alias: true` so the footer's hint row skips it,
/// since showing both would just repeat the same action twice. `F1` has no
/// counterpart, so it is not one — hiding it would remove it from the
/// footer entirely rather than leave a shorter, still-complete hint row.
///
/// INVARIANT: every row's `KeyPattern` requires `ctrl` or `sup` — see
/// `every_binding_requires_a_modifier` below. A printable key with no
/// modifier here would shadow ordinary text input, which is exactly the
/// defect this table replaced the leader to avoid.
pub const GLOBAL_BINDINGS: &[Binding<GlobalCommand>] = &[
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('b'), CTRL)],
        cmd: GlobalCommand::FocusExplorer,
        help: "explorer",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('b'), SUP)],
        cmd: GlobalCommand::FocusExplorer,
        help: "explorer",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('e'), CTRL)],
        cmd: GlobalCommand::FocusEditor,
        help: "editor",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('e'), SUP)],
        cmd: GlobalCommand::FocusEditor,
        help: "editor",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('t'), CTRL)],
        cmd: GlobalCommand::FocusTabs,
        help: "tabs",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('t'), SUP)],
        cmd: GlobalCommand::FocusTabs,
        help: "tabs",
        when: "",
        alias: true,
    },
    // `⌘R` deliberately has NO row here, unlike the other four focus
    // commands' pairs: `EDITOR_BINDINGS`' `RELOAD` chord already claims
    // `⌘R` (re-decode the active image) gated on `when: "image"`. This
    // table's rows are resolved unconditionally at stage 2, before any
    // pane (including the editor) ever sees the key — `when` on a pane
    // table only partitions collisions AMONG that pane's own rows, never
    // against a stage-2 row, since `resolve_in` never consults `when` at
    // all. Adding `⌘R` here would make Reload permanently unreachable by
    // keyboard on an image document. `^R` is unaffected — nothing else in
    // this crate binds it — so `FocusTitle` keeps only its `^` form.
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('r'), CTRL)],
        cmd: GlobalCommand::FocusTitle,
        help: "rename",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('k'), CTRL)],
        cmd: GlobalCommand::CollapseLeft,
        help: "hide pane",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('k'), SUP)],
        cmd: GlobalCommand::CollapseLeft,
        help: "hide pane",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('s'), SUP)],
        cmd: GlobalCommand::Save,
        help: "save",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::F1, Mods::NONE)],
        cmd: GlobalCommand::Help,
        help: "help",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('c'), CTRL)],
        cmd: GlobalCommand::QuitChord(QuitKey::CtrlC),
        help: "quit",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('d'), CTRL)],
        cmd: GlobalCommand::QuitChord(QuitKey::CtrlD),
        help: "quit",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('w'), CTRL)],
        cmd: GlobalCommand::CloseFile,
        help: "close",
        when: "",
        alias: false,
    },
    // `^1`-`^9` switch to the tab at that position; `^0` is the TENTH tab,
    // matching what the tab strip itself prints for the first ten tabs
    // (`opentabs::draw`'s `(idx + 1) % 10` shortcut digit). Ten near-identical
    // hints would flood the footer's hint row, so all ten stay `alias: true`
    // — still fully discoverable through the F1 Help doc, just not repeated
    // ten times in the footer.
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('1'), CTRL)],
        cmd: GlobalCommand::TabSwitch(0),
        help: "tab 1",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('2'), CTRL)],
        cmd: GlobalCommand::TabSwitch(1),
        help: "tab 2",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('3'), CTRL)],
        cmd: GlobalCommand::TabSwitch(2),
        help: "tab 3",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('4'), CTRL)],
        cmd: GlobalCommand::TabSwitch(3),
        help: "tab 4",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('5'), CTRL)],
        cmd: GlobalCommand::TabSwitch(4),
        help: "tab 5",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('6'), CTRL)],
        cmd: GlobalCommand::TabSwitch(5),
        help: "tab 6",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('7'), CTRL)],
        cmd: GlobalCommand::TabSwitch(6),
        help: "tab 7",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('8'), CTRL)],
        cmd: GlobalCommand::TabSwitch(7),
        help: "tab 8",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('9'), CTRL)],
        cmd: GlobalCommand::TabSwitch(8),
        help: "tab 9",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('0'), CTRL)],
        cmd: GlobalCommand::TabSwitch(9),
        help: "tab 10",
        when: "",
        alias: true,
    },
    // `^p`/`⌘p` are unclaimed across all six binding tables (`GLOBAL`,
    // `EDITOR`, `VIM`, `TABS`, `EXPLORER`, `EXPLORER_SEARCH` — see
    // `global_p_binding_is_not_already_bound_in_any_pane_table` below).
    // The label stays "reading" in both directions (unlike the quit/tab
    // rows above, which differ) so the footer's hint row never jumps as
    // the state flips.
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('p'), CTRL)],
        cmd: GlobalCommand::ToggleReadOnly,
        help: "reading",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('p'), SUP)],
        cmd: GlobalCommand::ToggleReadOnly,
        help: "reading",
        when: "",
        alias: true,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Structural proof of the invariant the module doc above states: a
    /// `Char` row (the only kind of row a printable keystroke could ever
    /// match — `F1` and any other non-`Char` `KeyCode` can never be typed
    /// as text, so they are exempt) always requires ctrl or sup. This is
    /// what makes "every printable keystroke is text" true by construction
    /// rather than by convention — the exact property the deleted held-
    /// space leader violated.
    #[test]
    fn every_printable_binding_requires_a_modifier() {
        use crate::binding::KeyMatch;
        for binding in GLOBAL_BINDINGS {
            for key in binding.keys {
                if !matches!(key.key, KeyMatch::Code(KeyCode::Char(_))) {
                    continue;
                }
                assert!(
                    key.mods.ctrl || key.mods.sup,
                    "{:?} has no ctrl/sup modifier and could shadow text input",
                    key
                );
            }
        }
    }

    /// Plan WP5: there is no cross-table keymap-union guard in this
    /// codebase — `index::validate` runs per-table only, and a
    /// `GLOBAL_BINDINGS` row resolves at stage 2, before any pane's own
    /// keymap ever sees the key (`resolve_in` never consults `when`), so a
    /// global row can silently shadow a pane binding with nothing to catch
    /// it. Modelled on `editor_bindings::reload_key_is_not_already_bound_
    /// elsewhere_in_the_editor_table`'s `⌘R` guard, widened across every
    /// pane table this crate has, since `^p`/`⌘p` mint a new global row
    /// rather than reusing an existing chord the way `⌘R` does.
    ///
    /// Checks the actual dispatch-time predicate, `KeyPattern::matches`, not
    /// structural equality on `keys` — a pane row does not need to equal
    /// `⌃P`/`⌘P` to steal them, it only needs to MATCH them, and
    /// `KeyMatch::Printable` (the Explorer type-to-search wildcard) matches
    /// any non-control `Char` under equal `Mods` without ever equaling a
    /// specific `KeyPattern`. Structural equality would stay green while
    /// that wildcard silently shadowed this exact chord at dispatch.
    #[test]
    fn global_p_binding_is_not_already_bound_in_any_pane_table() {
        use crate::explorer_keys::EXPLORER_BINDINGS;
        use crate::explorer_search::EXPLORER_SEARCH_BINDINGS;
        use crate::keymap::KeyInput;
        use crate::keymap::editor_bindings::EDITOR_BINDINGS;
        use crate::keymap::vim::VIM_BINDINGS;
        use crate::opentabs::TABS_BINDINGS;

        let ctrl_p = KeyInput {
            code: KeyCode::Char('p'),
            mods: CTRL,
        };
        let sup_p = KeyInput {
            code: KeyCode::Char('p'),
            mods: SUP,
        };

        fn claimants<C: Copy + 'static>(table: &[Binding<C>], key: KeyInput) -> Vec<&'static str> {
            table
                .iter()
                .filter(|b| b.keys.iter().any(|k| k.matches(key)))
                .map(|b| b.help)
                .collect()
        }

        for key in [ctrl_p, sup_p] {
            assert!(
                claimants(EDITOR_BINDINGS, key).is_empty(),
                "EDITOR_BINDINGS already binds {key:?}"
            );
            assert!(
                claimants(VIM_BINDINGS, key).is_empty(),
                "VIM_BINDINGS already binds {key:?}"
            );
            assert!(
                claimants(TABS_BINDINGS, key).is_empty(),
                "TABS_BINDINGS already binds {key:?}"
            );
            assert!(
                claimants(EXPLORER_BINDINGS, key).is_empty(),
                "EXPLORER_BINDINGS already binds {key:?}"
            );
            assert!(
                claimants(EXPLORER_SEARCH_BINDINGS, key).is_empty(),
                "EXPLORER_SEARCH_BINDINGS already binds {key:?}"
            );
        }
    }
}
