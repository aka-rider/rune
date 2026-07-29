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
    /// — free across `GLOBAL_BINDINGS`, `LEADER_BINDINGS`, `resolve_char`,
    /// `TABS_BINDINGS` and `EXPLORER_BINDINGS`). Global rather than
    /// editor-scoped so a rename is reachable from any pane, exactly like
    /// Save.
    FocusTitle,
    Save,
    Help,
    QuitChord(QuitKey),
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

/// `^b`/`^e`/`^t`/F1 are always-works ctrl fallbacks (plan WP5.S1: `^x`
/// retired as the explorer chord once the held-space leader — below — took
/// over as the primary way in; `^b` is free, see plan decision 6/risk R4).
/// `Save` and the two quit chords are the SAME combos `resolve`/
/// `QuitKey::from_key` already bind — moving their resolution to the global
/// pipeline stage changes only WHEN they're seen (before, not after, a
/// pane's own keymap), not which chord activates them. `KeyPattern`'s
/// exact-modifier match narrows `resolve_char`'s `'s' if m.sup && !m.ctrl`
/// guard (which also tolerated shift/alt held) to the one precise combo
/// below — the loosely-matched variants were never a documented,
/// intentional binding.
///
/// A ctrl chord that duplicates a leader chord (or, for `^d`, another quit
/// chord) is marked `alias: true` so the footer's hint row skips it, since
/// showing both would just repeat the same action twice. `F1` has no leader
/// equivalent, so it is not one — hiding it would remove it from the footer
/// entirely rather than leave a shorter, still-complete hint row.
pub const GLOBAL_BINDINGS: &[Binding<GlobalCommand>] = &[
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('b'), CTRL)],
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
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('t'), CTRL)],
        cmd: GlobalCommand::FocusTabs,
        help: "tabs",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('r'), CTRL)],
        cmd: GlobalCommand::FocusTitle,
        help: "rename",
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
];

/// The glyph every leader label is prefixed with — U+2423 OPEN BOX (plan
/// decision 9), standing in for the physically-held space that arms it.
pub const LEADER_GLYPH: char = '␣';

/// The held-space leader table (plan decisions 1/2/4): resolved only when
/// `app.space_probe.space_is_down()` confirms space is still physically
/// down at the instant one of these keys arrives (`app::handle_key`'s
/// leader stage). A plain `Binding<GlobalCommand>` table, not a bespoke
/// struct, so it reuses `resolve_in` verbatim and stays enumerable by the
/// same reflection the footer and Help doc already use over
/// `GLOBAL_BINDINGS`. `Mods::NONE` is load-bearing: `KeyPattern::matches`
/// compares the WHOLE `Mods` set, so `⌘X` (Cut) and `^X`/`^B` can never be
/// mistaken for a leader completion.
pub const LEADER_BINDINGS: &[Binding<GlobalCommand>] = &[
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('x'), Mods::NONE)],
        cmd: GlobalCommand::FocusExplorer,
        help: "explorer",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('e'), Mods::NONE)],
        cmd: GlobalCommand::FocusEditor,
        help: "editor",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('t'), Mods::NONE)],
        cmd: GlobalCommand::FocusTabs,
        help: "tabs",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('r'), Mods::NONE)],
        cmd: GlobalCommand::FocusTitle,
        help: "rename",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('z'), Mods::NONE)],
        cmd: GlobalCommand::CollapseLeft,
        help: "hide panel",
        when: "",
        alias: false,
    },
];

/// `␣X`-style label — the one source the footer and the Help doc both
/// render, so the two can never drift out of step with each other or with
/// `LEADER_BINDINGS` itself.
pub fn leader_label(b: &Binding<GlobalCommand>) -> String {
    format!("{LEADER_GLYPH}{}", b.label())
}
