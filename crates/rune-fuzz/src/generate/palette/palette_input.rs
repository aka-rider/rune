//! Input-corpus data split out of `palette.rs`: the key/chord palettes and
//! command-palette constants every `cluster_*` strategy in `cluster.rs`
//! draws from.

use rune_tui::keymap::{KeyCode, KeyInput, Mods};

/// The eight motions the `Navigate` cluster draws from, plus alt+Left/Right.
pub(in crate::generate) static NAV_KEYS: &[KeyInput] = &[
    KeyInput {
        code: KeyCode::Left,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Right,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Up,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Down,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Home,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::End,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::PageUp,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::PageDown,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Left,
        mods: Mods {
            shift: false,
            alt: true,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Right,
        mods: Mods {
            shift: false,
            alt: true,
            ctrl: false,
            sup: false,
        },
    },
];

/// The same eight motions, shift-modified, for the `Selection` cluster.
pub(in crate::generate) static SELECT_MOTION_KEYS: &[KeyInput] = &[
    KeyInput {
        code: KeyCode::Left,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Right,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Up,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Down,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Home,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::End,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::PageUp,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::PageDown,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
];

/// The `Delete` cluster's four keys.
pub(in crate::generate) static DELETE_KEYS: &[KeyInput] = &[
    KeyInput {
        code: KeyCode::Backspace,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Delete,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Tab,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Tab,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
];

pub(in crate::generate) const SELECT_ALL_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('a'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(in crate::generate) const UNDO_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('z'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(in crate::generate) const REDO_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('z'),
    mods: Mods {
        shift: true,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(in crate::generate) const REDO_KEY_ALT: KeyInput = KeyInput {
    code: KeyCode::Char('Z'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(in crate::generate) const NAV_BACK_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('['),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};
pub(in crate::generate) const NAV_FORWARD_KEY: KeyInput = KeyInput {
    code: KeyCode::Char(']'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};
pub(in crate::generate) const ENTER_KEY: KeyInput = KeyInput {
    code: KeyCode::Enter,
    mods: Mods::NONE,
};
pub(in crate::generate) const SAVE_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('s'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(in crate::generate) const COPY_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('c'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(in crate::generate) const CUT_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('x'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(in crate::generate) const PASTE_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('v'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(in crate::generate) const CTRL_C_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('c'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

/// `^r` (`GlobalCommand::FocusTitle`) — reaching `Pane::Title` is what
/// extends `PANE-NO-BLEED` to cover "typing a filename never touches a
/// buffer byte". Every subsequent generated character then lands in the
/// title field instead of the document, which is precisely the property
/// worth fuzzing.
pub(in crate::generate) const CTRL_R_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('r'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

/// ⌥←/⌥→ (word motion), ⇧←/⇧→ (shift-selection) and `UNDO_KEY` (⌘Z) —
/// paired with `CTRL_R_KEY` by `cluster_chrome` so a single generated
/// cluster both parks focus on the title AND immediately exercises one of
/// its own editing bindings, resolved through the SAME `EDITOR_BINDINGS`
/// table the document editor uses (plan WP3 decision 3). `⌥←`/`⌥→` are
/// deliberately the plain word-motion pair, not the shift-word variant —
/// `SELECT_MOTION_KEYS` already covers the char-wise shift pair, and
/// `PANE-NO-BLEED` cares about "the title, not the document, moved",
/// which any of these five keys equally proves.
pub(in crate::generate) static TITLE_MOTION_KEYS: &[KeyInput] = &[
    KeyInput {
        code: KeyCode::Left,
        mods: Mods {
            shift: false,
            alt: true,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Right,
        mods: Mods {
            shift: false,
            alt: true,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Left,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Right,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    UNDO_KEY,
];

/// Plain `Esc`, no mods — the key `banner::handle_key`'s stage 1 uses to
/// clear the modal Guard without
/// touching a buffer byte, and the key `title::handle_key` uses to revert
/// and hand focus back to the editor. `driver/checks.rs::
/// restore_editor_focus` is the existing precedent for this exact key.
pub(in crate::generate) const ESCAPE_KEY: KeyInput = KeyInput {
    code: KeyCode::Escape,
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: false,
    },
};

/// `^b` (`GlobalCommand::ToggleLeft`) — CODE-REVIEW.md rune-fuzz
/// finding 10: without this, `DirLoaded` always lands in a never-opened
/// Explorer (the cursor-preserving Refresh path is only ever exercised
/// against empty state), and the whole Explorer pane was reachable only by
/// the ~1e-10-probability monkey-burst cluster. Also doubles as the
/// Enter/Escape rework's own way back to the Editor from any pane the
/// column can hold (`Pane::Explorer`/`Pane::Tabs`), since both can only
/// ever be focused while the column is painted.
pub(in crate::generate) const CTRL_B_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('b'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

/// `^t` (`GlobalCommand::FocusTabs`) — the Tabs pane's own equivalent of
/// `CTRL_B_KEY` above (CODE-REVIEW.md rune-fuzz finding 10).
pub(in crate::generate) const CTRL_T_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('t'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

/// `^p` (`GlobalCommand::ToggleReadOnly`) — the reading-view toggle (plan
/// WP5/WP8). Reaching it is what exercises the reveal-follows-insertion-
/// point machinery (plan WP1) against the fuzzer's own generated
/// documents, not just the deterministic test suite.
pub(in crate::generate) const CTRL_P_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('p'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

/// `^e` (`GlobalCommand::ToggleMessages`) — the message pane's own
/// open/focus/collapse toggle. Reaching it is what exercises the pane
/// (and, by extension, `Pane::Messages` focus routing) against the
/// fuzzer's own generated sessions, not just the deterministic test suite.
pub(in crate::generate) const CTRL_E_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('e'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

/// `⌘⌫` (`GlobalCommand::Trash`) — the recoverable-delete chord. Reaching
/// it reliably is what exercises the trash guard and the async
/// `CmdKind::Trash` discharge against the fuzzer's own generated sessions,
/// not just the deterministic test suite.
pub(in crate::generate) const TRASH_KEY: KeyInput = KeyInput {
    code: KeyCode::Backspace,
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};

/// `^⇧F` (`GlobalCommand::ToggleFileSearch`, primary binding) — the fuzzy
/// file finder's own chord. Without this, the finder's whole open/typing/
/// close surface (and the close-gate/focus invariants it must hold under
/// every other global command) was reachable only through `cluster_monkey_
/// burst`'s ~0.4%-of-16-mods-per-key odds, the same reachability gap
/// `CTRL_B_KEY`/`CTRL_T_KEY` closed for the Explorer/Tabs panes.
pub(in crate::generate) const FILESEARCH_KEY_CTRL: KeyInput = KeyInput {
    code: KeyCode::Char('F'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

/// `⌘⇧F`, the same command's alias binding.
pub(in crate::generate) const FILESEARCH_KEY_SUP: KeyInput = KeyInput {
    code: KeyCode::Char('F'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};

/// Unmodified printable-letter keys for the Explorer type-to-search feature
/// (`explorer_search.rs`): `KeyPattern::printable`'s wildcard row matches
/// any non-control `Char` under `Mods::NONE`, so this
/// is only ever exercised if a generated key actually reaches the Explorer
/// AS an unmodified letter — `cluster_chrome`'s `^b`-then-type arm
/// (`cluster.rs`) is what supplies that reachability, letting `PANE-NO-
/// BLEED` prove a key aimed at the Explorer (moving `nav.cursor`, not the
/// active document) never mutates a buffer byte.
pub(in crate::generate) static EXPLORER_SEARCH_KEYS: &[KeyInput] = &[
    KeyInput {
        code: KeyCode::Char('r'),
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Char('e'),
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Char('a'),
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Char('z'),
        mods: Mods::NONE,
    },
];

/// `alt+sup+Up` (`Command::AddCursorAbove`, `keymap/editor_bindings.rs`'s
/// `ALT_SUP` row) — CODE-REVIEW.md rune-fuzz finding 11: the entire
/// multi-cursor surface was monkey-burst-only at ~0.42%/key, so
/// `cluster_multicursor` (`cluster.rs`) can actually build a session with
/// more than one cursor to check `CUR-ORDER`/clipboard against.
pub(in crate::generate) const ADD_CURSOR_ABOVE_KEY: KeyInput = KeyInput {
    code: KeyCode::Up,
    mods: Mods {
        shift: false,
        alt: true,
        ctrl: false,
        sup: true,
    },
};

/// `alt+sup+Down` (`Command::AddCursorBelow`) — the downward twin of
/// `ADD_CURSOR_ABOVE_KEY` above.
pub(in crate::generate) const ADD_CURSOR_BELOW_KEY: KeyInput = KeyInput {
    code: KeyCode::Down,
    mods: Mods {
        shift: false,
        alt: true,
        ctrl: false,
        sup: true,
    },
};

/// `^M` (`GlobalCommand::Merge`, plan WP7.S1): every fuzz session now opens
/// its seeded document through a real, in-memory-backed recovery store, so
/// `merge::begin`'s own fast pre-check can genuinely find divergence —
/// `cluster_merge` (`cluster.rs`) always presses this AFTER an
/// `Action::DivergeDisk` and its reprobe ack land, so this chord routinely
/// carries a session all the way into `MergeState::Active`, not just its
/// refusal path.
pub(in crate::generate) const MERGE_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('m'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

pub(in crate::generate) const CMDPAL_KEY_CTRL: KeyInput = KeyInput {
    code: KeyCode::Char('P'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

pub(in crate::generate) const CMDPAL_KEY_SUP: KeyInput = KeyInput {
    code: KeyCode::Char('P'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};

pub(in crate::generate) const CMDPAL_TAB_KEY: KeyInput = KeyInput {
    code: KeyCode::Tab,
    mods: Mods::NONE,
};

pub(in crate::generate) const CMDPAL_BACKSPACE_KEY: KeyInput = KeyInput {
    code: KeyCode::Backspace,
    mods: Mods::NONE,
};

pub(in crate::generate) static CMDPAL_NAV_KEYS: &[KeyInput] = &[
    KeyInput {
        code: KeyCode::Up,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Down,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::PageUp,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::PageDown,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Home,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::End,
        mods: Mods::NONE,
    },
];

pub(in crate::generate) static CMDPAL_PARAM_QUERIES: &[&str] = &["lang", "tab"];

/// `⇧⌘Y`/`⇧⌘U` (`DiffCommand::TakeTheirs`/`TakeOurs`, `diff_view::keys::
/// DIFF_BINDINGS`) — the pane verb layer's own conflict-resolving chords.
/// `cluster_merge` presses exactly one of these right after the working
/// form installs, so it lands on a genuine resolution most of the time
/// rather than always falling through to plain-char insertion; a session
/// that never reaches `Active` (a `DiskAhead` clean fast path, a UTF-8
/// refusal, ...) still exercises the fallthrough itself, which is exactly
/// `MERGE-KEY-FEEDBACK`'s other half.
pub(in crate::generate) static MERGE_RESOLVE_KEYS: &[KeyInput] = &[
    KeyInput {
        code: KeyCode::Char('y'),
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: true,
        },
    },
    KeyInput {
        code: KeyCode::Char('u'),
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: true,
        },
    },
    KeyInput {
        code: KeyCode::Char('j'),
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: true,
        },
    },
    KeyInput {
        code: KeyCode::Char('k'),
        mods: Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    },
];
