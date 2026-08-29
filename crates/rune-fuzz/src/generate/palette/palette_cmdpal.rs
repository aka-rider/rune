use rune_tui::keymap::{KeyCode, KeyInput, Mods};

pub(in crate::generate) const CMDPAL_KEY_CTRL: KeyInput = KeyInput {
    code: KeyCode::Char('p'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

pub(in crate::generate) const CMDPAL_KEY_SUP: KeyInput = KeyInput {
    code: KeyCode::Char('p'),
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

pub(in crate::generate) const MERGE_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('m'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

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
