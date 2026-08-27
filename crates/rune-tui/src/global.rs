use crate::binding::{Binding, KeyPattern};
use crate::keymap::{KeyCode, Mods, QuitKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlobalCommand {
    ToggleLeft,
    FocusTabs,
    FocusTitle,
    Save,
    Help,
    QuitChord(QuitKey),
    CloseFile,
    NewDocument,
    TabSwitch(usize),
    ToggleReadOnly,
    Merge,
    ToggleMessages,
    Trash,
    ToggleSearch,
    SearchNext,
    SearchPrev,
    TogglePin,
    ToggleFileSearch,
    TogglePalette,
    NavBack,
    NavForward,
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

pub const GLOBAL_BINDINGS: &[Binding<GlobalCommand>] = &[
    Binding {
        key: KeyPattern::new(KeyCode::Char('b'), CTRL),
        cmd: GlobalCommand::ToggleLeft,
        help: "explorer",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('b'), SUP),
        cmd: GlobalCommand::ToggleLeft,
        help: "explorer",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('t'), CTRL),
        cmd: GlobalCommand::FocusTabs,
        help: "tabs",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('t'), SUP),
        cmd: GlobalCommand::FocusTabs,
        help: "tabs",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('r'), CTRL),
        cmd: GlobalCommand::FocusTitle,
        help: "rename",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('s'), CTRL),
        cmd: GlobalCommand::Save,
        help: "save",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('s'), SUP),
        cmd: GlobalCommand::Save,
        help: "save",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::F1, Mods::NONE),
        cmd: GlobalCommand::Help,
        help: "help",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('c'), CTRL),
        cmd: GlobalCommand::QuitChord(QuitKey::CtrlC),
        help: "quit",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('d'), CTRL),
        cmd: GlobalCommand::QuitChord(QuitKey::CtrlD),
        help: "quit",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('w'), CTRL),
        cmd: GlobalCommand::CloseFile,
        help: "close",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('n'), CTRL),
        cmd: GlobalCommand::NewDocument,
        help: "new",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('n'), SUP),
        cmd: GlobalCommand::NewDocument,
        help: "new",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('1'), CTRL),
        cmd: GlobalCommand::TabSwitch(0),
        help: "tab 1",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('2'), CTRL),
        cmd: GlobalCommand::TabSwitch(1),
        help: "tab 2",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('3'), CTRL),
        cmd: GlobalCommand::TabSwitch(2),
        help: "tab 3",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('4'), CTRL),
        cmd: GlobalCommand::TabSwitch(3),
        help: "tab 4",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('5'), CTRL),
        cmd: GlobalCommand::TabSwitch(4),
        help: "tab 5",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('6'), CTRL),
        cmd: GlobalCommand::TabSwitch(5),
        help: "tab 6",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('7'), CTRL),
        cmd: GlobalCommand::TabSwitch(6),
        help: "tab 7",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('8'), CTRL),
        cmd: GlobalCommand::TabSwitch(7),
        help: "tab 8",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('9'), CTRL),
        cmd: GlobalCommand::TabSwitch(8),
        help: "tab 9",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('0'), CTRL),
        cmd: GlobalCommand::TabSwitch(9),
        help: "tab 10",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('p'), CTRL),
        cmd: GlobalCommand::ToggleReadOnly,
        help: "reading",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('p'), SUP),
        cmd: GlobalCommand::ToggleReadOnly,
        help: "reading",
        secondary: true,
    },
    // `^M` only, no `⌘M`: Ghostty steals `⌘M` for window minimize before
    // the app ever sees it. `^M` is safe because this app requests the
    // kitty CSI-u protocol, under which `termina` decodes Ctrl+M as
    // `Char('m')` with `CONTROL` set, distinct from `Enter` (code 13); a
    // terminal that never negotiates the protocol reports `^M` as plain
    // `Enter`, so this binding simply never fires there.
    Binding {
        key: KeyPattern::new(KeyCode::Char('m'), CTRL),
        cmd: GlobalCommand::Merge,
        help: "merge",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('e'), CTRL),
        cmd: GlobalCommand::ToggleMessages,
        help: "messages",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('e'), SUP),
        cmd: GlobalCommand::ToggleMessages,
        help: "messages",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('f'), CTRL),
        cmd: GlobalCommand::ToggleSearch,
        help: "search",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('f'), SUP),
        cmd: GlobalCommand::ToggleSearch,
        help: "search",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('g'), CTRL),
        cmd: GlobalCommand::SearchNext,
        help: "next match",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('g'), SUP),
        cmd: GlobalCommand::SearchNext,
        help: "next match",
        secondary: true,
    },
    // This crate requests `REPORT_ALTERNATE_KEYS`, under which a shifted
    // chord arrives as the shifted character with `SHIFT` itself cleared
    // — so `SearchPrev`/`ToggleFileSearch` below bind the shifted char
    // (`'G'`/`'F'`), not the base char with a `SHIFT` bit set.
    Binding {
        key: KeyPattern::new(KeyCode::Char('G'), CTRL),
        cmd: GlobalCommand::SearchPrev,
        help: "prev match",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('G'), SUP),
        cmd: GlobalCommand::SearchPrev,
        help: "prev match",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('F'), CTRL),
        cmd: GlobalCommand::ToggleFileSearch,
        help: "find file",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('F'), SUP),
        cmd: GlobalCommand::ToggleFileSearch,
        help: "find file",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('P'), CTRL),
        cmd: GlobalCommand::TogglePalette,
        help: "command palette",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('P'), SUP),
        cmd: GlobalCommand::TogglePalette,
        help: "command palette",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('j'), CTRL),
        cmd: GlobalCommand::TogglePin,
        help: "pin",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('['), CTRL),
        cmd: GlobalCommand::NavBack,
        help: "back",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('['), SUP),
        cmd: GlobalCommand::NavBack,
        help: "back",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char(']'), CTRL),
        cmd: GlobalCommand::NavForward,
        help: "forward",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char(']'), SUP),
        cmd: GlobalCommand::NavForward,
        help: "forward",
        secondary: true,
    },
];

pub fn hint_for(cmd: GlobalCommand) -> Option<(String, &'static str)> {
    canonical(cmd).map(|b| (b.label(), b.help))
}

pub(crate) fn label_for(cmd: GlobalCommand) -> String {
    hint_for(cmd).map(|(label, _)| label).unwrap_or_default()
}

fn canonical(cmd: GlobalCommand) -> Option<&'static Binding<GlobalCommand>> {
    GLOBAL_BINDINGS
        .iter()
        .find(|b| !b.secondary && b.cmd == cmd)
}

#[cfg(test)]
#[path = "global_tests.rs"]
mod tests;
