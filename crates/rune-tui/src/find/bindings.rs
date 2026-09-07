use crate::binding::{Binding, KeyPattern};
use crate::keymap::{KeyCode, Mods};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindCommand {
    Type,
    Erase,
    Close,
    Commit,
    Alt,
    NextControl,
    PrevControl,
    Activate,
    ToggleCase,
    ToggleWord,
    ToggleRegex,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

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

pub const FIND_BINDINGS: &[Binding<FindCommand>] = &[
    Binding {
        key: KeyPattern::new(KeyCode::Char(' '), Mods::NONE),
        cmd: FindCommand::Activate,
        help: "toggle",
        secondary: false,
    },
    Binding {
        key: KeyPattern::printable(Mods::NONE),
        cmd: FindCommand::Type,
        help: "type to search",
        secondary: false,
    },
    Binding {
        key: KeyPattern::printable(SHIFT),
        cmd: FindCommand::Type,
        help: "type to search",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Backspace, Mods::NONE),
        cmd: FindCommand::Erase,
        help: "erase",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Escape, Mods::NONE),
        cmd: FindCommand::Close,
        help: "close",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Enter, Mods::NONE),
        cmd: FindCommand::Commit,
        help: "next match",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Enter, SHIFT),
        cmd: FindCommand::Alt,
        help: "previous match",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Tab, Mods::NONE),
        cmd: FindCommand::NextControl,
        help: "next control",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Tab, SHIFT),
        cmd: FindCommand::PrevControl,
        help: "previous control",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('c'), ALT),
        cmd: FindCommand::ToggleCase,
        help: "match case",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('w'), ALT),
        cmd: FindCommand::ToggleWord,
        help: "whole word",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('r'), ALT),
        cmd: FindCommand::ToggleRegex,
        help: "regex",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Up, Mods::NONE),
        cmd: FindCommand::Up,
        help: "history",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Down, Mods::NONE),
        cmd: FindCommand::Down,
        help: "newer",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::PageUp, Mods::NONE),
        cmd: FindCommand::PageUp,
        help: "results page up",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::PageDown, Mods::NONE),
        cmd: FindCommand::PageDown,
        help: "results page down",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Home, Mods::NONE),
        cmd: FindCommand::Home,
        help: "first result",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::End, Mods::NONE),
        cmd: FindCommand::End,
        help: "last result",
        secondary: false,
    },
];

pub(crate) fn label_for(cmd: FindCommand) -> String {
    FIND_BINDINGS
        .iter()
        .find(|b| !b.secondary && b.cmd == cmd)
        .map(Binding::label)
        .unwrap_or_default()
}
