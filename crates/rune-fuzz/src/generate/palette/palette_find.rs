use rune_tui::keymap::{KeyCode, KeyInput, Mods};

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

const ALT: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: false,
};

pub(in crate::generate) const FIND_KEY_CTRL: KeyInput = KeyInput {
    code: KeyCode::Char('f'),
    mods: CTRL,
};

pub(in crate::generate) const REPLACE_KEY_CTRL: KeyInput = KeyInput {
    code: KeyCode::Char('r'),
    mods: CTRL,
};

pub(in crate::generate) static FIND_PANEL_KEYS: &[KeyInput] = &[
    KeyInput {
        code: KeyCode::Tab,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Tab,
        mods: SHIFT,
    },
    KeyInput {
        code: KeyCode::Char(' '),
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Char('c'),
        mods: ALT,
    },
    KeyInput {
        code: KeyCode::Char('w'),
        mods: ALT,
    },
    KeyInput {
        code: KeyCode::Char('r'),
        mods: ALT,
    },
    KeyInput {
        code: KeyCode::Enter,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Enter,
        mods: SHIFT,
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
        code: KeyCode::Backspace,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Char('g'),
        mods: CTRL,
    },
];

pub(in crate::generate) static FIND_CHARS: &[char] =
    &['a', 'e', 'o', 't', 'h', 'n', ' ', '.', '(', '*', '$', '1'];
