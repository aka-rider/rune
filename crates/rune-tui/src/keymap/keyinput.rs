#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Backspace,
    Tab,
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
    F1,
}

// Field names avoid `super` (a reserved path keyword) and spell out `sup`
// for the Command/Super key — Command on macOS, the platform this app
// exclusively targets.
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

// termina's own docs say code handling shortcuts should usually check
// `kind == KeyEventKind::Press`; this treats `Repeat` the same as `Press`
// so a held arrow key keeps moving, and drops `Release` entirely.
pub fn from_termina(event: termina::event::KeyEvent) -> Option<KeyInput> {
    use termina::event::{KeyCode as TK, KeyEventKind, Modifiers as TM};

    if !matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }

    let shift_tab = matches!(event.code, TK::BackTab);

    let code = match event.code {
        TK::Char(c) => KeyCode::Char(c),
        TK::Enter => KeyCode::Enter,
        TK::Backspace => KeyCode::Backspace,
        TK::Tab => KeyCode::Tab,
        TK::BackTab => KeyCode::Tab,
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
        shift: m.contains(TM::SHIFT) || shift_tab,
        alt: m.contains(TM::ALT),
        ctrl: m.contains(TM::CONTROL),
        sup: m.contains(TM::SUPER),
    };
    Some(KeyInput { code, mods })
}
