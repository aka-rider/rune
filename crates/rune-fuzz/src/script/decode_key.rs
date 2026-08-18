use super::ScriptError;
use super::decode::unescape;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

/// Splits a `key` line's remainder into code and mods fields by taking the
/// LAST 4 characters as mods and the character before that as the
/// separator — never by a generic whitespace split (module docs).
pub(super) fn parse_key(rest: &str, line: usize) -> Result<KeyInput, ScriptError> {
    let malformed = |reason: &str| ScriptError::MalformedLine {
        line,
        reason: reason.to_string(),
    };
    let chars: Vec<char> = rest.chars().collect();
    let n = chars.len();
    if n < 5 {
        return Err(malformed("key line too short to contain a mods field"));
    }
    let mods_start = n - 4;
    let sep_idx = n - 5;

    let mods_chars = chars
        .get(mods_start..)
        .ok_or_else(|| malformed("could not read mods field"))?;
    let sep_char = *chars
        .get(sep_idx)
        .ok_or_else(|| malformed("could not read separator before mods field"))?;
    let code_chars = chars
        .get(..sep_idx)
        .ok_or_else(|| malformed("could not read code field"))?;
    if sep_char != ' ' {
        return Err(malformed(&format!(
            "expected a space before the mods field, found {sep_char:?}"
        )));
    }

    let mods_str: String = mods_chars.iter().collect();
    let code_str: String = code_chars.iter().collect();
    Ok(KeyInput {
        code: parse_code(&code_str, line)?,
        mods: parse_mods(&mods_str, line)?,
    })
}

fn parse_mods(s: &str, line: usize) -> Result<Mods, ScriptError> {
    let invalid = || ScriptError::InvalidMods {
        line,
        mods: s.to_string(),
    };
    let chars: Vec<char> = s.chars().collect();
    if chars.len() != 4 {
        return Err(invalid());
    }
    let flag = |idx: usize, on: char| match chars.get(idx) {
        Some('-') => Ok(false),
        Some(&c) if c == on => Ok(true),
        _ => Err(invalid()),
    };
    Ok(Mods {
        shift: flag(0, 's')?,
        alt: flag(1, 'a')?,
        ctrl: flag(2, 'c')?,
        sup: flag(3, 'u')?,
    })
}

/// Mirrors `encode::encode_code`'s spellings; the `_` arm is the only place
/// a `char:` payload is accepted.
fn parse_code(s: &str, line: usize) -> Result<KeyCode, ScriptError> {
    let code = match s {
        "enter" => KeyCode::Enter,
        "backspace" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        "escape" => KeyCode::Escape,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "delete" => KeyCode::Delete,
        "f1" => KeyCode::F1,
        _ => {
            let invalid = || ScriptError::InvalidKeyCode {
                line,
                code: s.to_string(),
            };
            let escaped = s.strip_prefix("char:").ok_or_else(invalid)?;
            let unescaped = unescape(escaped, line)?;
            let mut it = unescaped.chars();
            let c = it.next().ok_or_else(invalid)?;
            if it.next().is_some() {
                return Err(invalid());
            }
            KeyCode::Char(c)
        }
    };
    Ok(code)
}
