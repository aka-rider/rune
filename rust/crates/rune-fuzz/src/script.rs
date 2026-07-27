//! A hand-written, dependency-free line codec for `(content, Vec<Action>)`.
//! One action per line, first line always `content <escaped>`:
//!
//! ```text
//! content <escaped>            # always the first line
//! key <code> <mods>            # key char:a ----   |  key left s---  |  key char:\u{20} ---u
//! type <escaped>
//! paste <escaped>
//! resize <w> <h>
//! clip <escaped>
//! confirm-timeout
//! deliver
//! fail-next-save
//! dirloaded <nav|refresh> <generation>   # followed by 0+ continuation lines:
//! dirloaded-entry <f|d> <escaped name>
//! ```
//!
//! `dirloaded`/`dirloaded-entry` is the one MULTI-line action (plan
//! WP4.S6): a `DirEntry`'s `name` is an arbitrary `String` that may itself
//! contain a literal space, so packing a variable-length entry list onto
//! one line with a space-joined delimiter would be ambiguous — one
//! `dirloaded-entry` continuation line per entry sidesteps that instead of
//! inventing a second escaping scheme.
//!
//! Deviation from the plan's grammar sketch: `Action` (`crate::action`) has
//! no `DeliverMode` — G9 proves at most one save can ever be outstanding —
//! so `deliver` is a bare token, never `deliver oldest|newest|all`.
//!
//! `<mods>` is a fixed 4-char field (shift, alt, ctrl, sup), each `-` or its
//! initial letter. Decode locates it by taking the LAST 4 characters of the
//! line plus the separating space before them, never by a generic
//! whitespace split — so an escaped `char:` payload may itself contain a
//! literal space with no ambiguity.
//!
//! Text payloads are escaped with `char::escape_default()` — always ASCII:
//! printable ASCII passes through unescaped, everything else becomes one of
//! `\n \r \t \\ \' \"` or a `\u{HEX}` run. `unescape` accepts exactly that
//! set, plus `\0` (which `escape_default` itself emits as `\u{0}`, but a
//! hand-authored script may still use the mnemonic).
//!
//! No `unwrap`/`expect`/`panic!`/unchecked indexing in this file (G17) —
//! every fallible step returns a `ScriptError`, mirroring
//! `rune_core::buffer::BufferError`'s idiom.

use std::fmt;
use std::iter::Peekable;

use crate::action::Action;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::DirCause;
use rune_vfs::DirEntry;

/// Why a script line could not be decoded. Never constructed by `encode` —
/// only ever returned by `decode` on malformed input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptError {
    /// No non-comment, non-blank `content` line was found.
    MissingContentLine,
    /// A line was structurally wrong in a way no more specific variant names.
    MalformedLine { line: usize, reason: String },
    /// An action line's first token matched no known keyword.
    UnknownKeyword { line: usize, keyword: String },
    /// A `\`-escape was unknown, truncated, or an invalid `\u{...}` value.
    InvalidEscape { line: usize, reason: String },
    /// A `key` line's code field matched no known `KeyCode` spelling.
    InvalidKeyCode { line: usize, code: String },
    /// A `key` line's mods field was not exactly 4 valid flag characters.
    InvalidMods { line: usize, mods: String },
    /// A numeric field did not parse as its expected integer type.
    InvalidNumber { line: usize, reason: String },
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptError::MissingContentLine => write!(f, "script has no `content` line"),
            ScriptError::MalformedLine { line, reason } => write!(f, "line {line}: {reason}"),
            ScriptError::UnknownKeyword { line, keyword } => {
                write!(f, "line {line}: unknown action keyword {keyword:?}")
            }
            ScriptError::InvalidEscape { line, reason } => write!(f, "line {line}: {reason}"),
            ScriptError::InvalidKeyCode { line, code } => {
                write!(f, "line {line}: invalid key code {code:?}")
            }
            ScriptError::InvalidMods { line, mods } => {
                write!(f, "line {line}: invalid mods field {mods:?}")
            }
            ScriptError::InvalidNumber { line, reason } => write!(f, "line {line}: {reason}"),
        }
    }
}

impl std::error::Error for ScriptError {}

/// Encodes `(content, actions)` as script text, one action per line.
pub fn encode(content: &str, actions: &[Action]) -> String {
    let mut out = String::new();
    out.push_str("content ");
    out.push_str(&escape(content));
    out.push('\n');
    for action in actions {
        encode_action(&mut out, action);
    }
    out
}

fn encode_action(out: &mut String, action: &Action) {
    match action {
        Action::Key(k) => {
            out.push_str("key ");
            out.push_str(&encode_code(k.code));
            out.push(' ');
            out.push_str(&encode_mods(k.mods));
            out.push('\n');
        }
        Action::Type(s) => {
            out.push_str("type ");
            out.push_str(&escape(s));
            out.push('\n');
        }
        Action::Paste(s) => {
            out.push_str("paste ");
            out.push_str(&escape(s));
            out.push('\n');
        }
        Action::Resize(w, h) => out.push_str(&format!("resize {w} {h}\n")),
        Action::ClipboardReply(s) => {
            out.push_str("clip ");
            out.push_str(&escape(s));
            out.push('\n');
        }
        Action::ConfirmTimeout => out.push_str("confirm-timeout\n"),
        Action::Deliver => out.push_str("deliver\n"),
        Action::FailNextSave => out.push_str("fail-next-save\n"),
        Action::DirLoaded {
            entries,
            cause,
            generation,
        } => {
            out.push_str("dirloaded ");
            out.push_str(encode_dir_cause(*cause));
            out.push(' ');
            out.push_str(&generation.to_string());
            out.push('\n');
            for entry in entries {
                out.push_str("dirloaded-entry ");
                out.push(if entry.is_dir { 'd' } else { 'f' });
                out.push(' ');
                out.push_str(&escape(&entry.name));
                out.push('\n');
            }
        }
    }
}

fn encode_dir_cause(cause: DirCause) -> &'static str {
    match cause {
        DirCause::Nav => "nav",
        DirCause::Refresh => "refresh",
    }
}

/// Exhaustive on `KeyCode` (compiler-checked: adding a variant breaks this
/// build until it's handled here and in `parse_code`'s mirror match).
fn encode_code(code: KeyCode) -> String {
    match code {
        KeyCode::Char(c) => format!("char:{}", escape_char(c)),
        KeyCode::Enter => "enter".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => "backtab".into(),
        KeyCode::Escape => "escape".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::F1 => "f1".into(),
    }
}

fn encode_mods(m: Mods) -> String {
    let mut s = String::with_capacity(4);
    s.push(if m.shift { 's' } else { '-' });
    s.push(if m.alt { 'a' } else { '-' });
    s.push(if m.ctrl { 'c' } else { '-' });
    s.push(if m.sup { 'u' } else { '-' });
    s
}

fn escape_char(c: char) -> String {
    c.escape_default().collect()
}

fn escape(s: &str) -> String {
    s.chars().flat_map(|c| c.escape_default()).collect()
}

/// Hand-written unescape, the inverse of `escape`/`escape_char`. Handles
/// exactly `\n \r \t \\ \' \" \0 \u{...}`. `line` attributes an error to the
/// right source line.
fn unescape(s: &str, line: usize) -> Result<String, ScriptError> {
    let invalid = |reason: String| ScriptError::InvalidEscape { line, reason };
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('\'') => out.push('\''),
            Some('"') => out.push('"'),
            Some('0') => out.push('\0'),
            Some('u') => {
                if chars.next() != Some('{') {
                    return Err(invalid("truncated \\u escape".to_string()));
                }
                let mut hex = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(h) => hex.push(h),
                        None => return Err(invalid("truncated \\u escape".to_string())),
                    }
                }
                let cp = u32::from_str_radix(&hex, 16)
                    .map_err(|_| invalid(format!("invalid unicode escape \\u{{{hex}}}")))?;
                let ch = char::from_u32(cp)
                    .ok_or_else(|| invalid(format!("invalid unicode escape \\u{{{hex}}}")))?;
                out.push(ch);
            }
            Some(other) => return Err(invalid(format!("unknown escape sequence \\{other}"))),
            None => return Err(invalid("truncated escape sequence".to_string())),
        }
    }
    Ok(out)
}

/// Decodes script text produced by `encode` (or hand-written in the same
/// grammar). Blank lines and lines whose first non-space character is `#`
/// are comments, skipped wherever they appear.
pub fn decode(text: &str) -> Result<(String, Vec<Action>), ScriptError> {
    let mut content: Option<String> = None;
    let mut actions = Vec::new();
    let mut lines = text.lines().enumerate().peekable();

    while let Some((idx, raw)) = lines.next() {
        let line = idx + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if content.is_none() {
            let rest = raw
                .strip_prefix("content ")
                .ok_or_else(|| ScriptError::MalformedLine {
                    line,
                    reason: "expected the first non-comment line to start with `content `".into(),
                })?;
            content = Some(unescape(rest, line)?);
            continue;
        }

        if let Some(rest) = raw.strip_prefix("dirloaded ") {
            actions.push(parse_dir_loaded(rest, line, &mut lines)?);
            continue;
        }

        actions.push(parse_action_line(raw, line)?);
    }

    let content = content.ok_or(ScriptError::MissingContentLine)?;
    Ok((content, actions))
}

/// Parses a `dirloaded <cause>` line plus its `dirloaded-entry` continuation
/// lines (module docs) — the one multi-line action in this grammar, so this
/// is the only parser that needs to look ahead in `lines`.
fn parse_dir_loaded<'a>(
    rest: &str,
    line: usize,
    lines: &mut Peekable<impl Iterator<Item = (usize, &'a str)>>,
) -> Result<Action, ScriptError> {
    let malformed = || ScriptError::MalformedLine {
        line,
        reason: "expected `dirloaded <nav|refresh> <generation>`".to_string(),
    };
    let mut parts = rest.trim().splitn(2, ' ');
    let cause = match parts.next().ok_or_else(malformed)? {
        "nav" => DirCause::Nav,
        "refresh" => DirCause::Refresh,
        other => {
            return Err(ScriptError::MalformedLine {
                line,
                reason: format!("unknown dirloaded cause {other:?}"),
            });
        }
    };
    let generation: u32 = parts
        .next()
        .ok_or_else(malformed)?
        .trim()
        .parse()
        .map_err(|_| ScriptError::MalformedLine {
            line,
            reason: "expected a u32 generation".to_string(),
        })?;

    let mut entries = Vec::new();
    while let Some(&(_, next_raw)) = lines.peek() {
        let Some(entry_rest) = next_raw.strip_prefix("dirloaded-entry ") else {
            break;
        };
        let entry_line = match lines.next() {
            Some((idx, _)) => idx + 1,
            None => break,
        };
        entries.push(parse_dir_entry(entry_rest, entry_line)?);
    }

    Ok(Action::DirLoaded {
        entries,
        cause,
        generation,
    })
}

fn parse_dir_entry(rest: &str, line: usize) -> Result<DirEntry, ScriptError> {
    let malformed = || ScriptError::MalformedLine {
        line,
        reason: "expected `dirloaded-entry <f|d> <name>`".to_string(),
    };
    let mut parts = rest.splitn(2, ' ');
    let flag = parts.next().ok_or_else(malformed)?;
    let name_field = parts.next().ok_or_else(malformed)?;
    let is_dir = match flag {
        "d" => true,
        "f" => false,
        _ => return Err(malformed()),
    };
    Ok(DirEntry {
        name: unescape(name_field, line)?,
        is_dir,
    })
}

fn parse_action_line(raw: &str, line: usize) -> Result<Action, ScriptError> {
    if raw == "confirm-timeout" {
        return Ok(Action::ConfirmTimeout);
    }
    if raw == "deliver" {
        return Ok(Action::Deliver);
    }
    if raw == "fail-next-save" {
        return Ok(Action::FailNextSave);
    }
    if let Some(rest) = raw.strip_prefix("type ") {
        return Ok(Action::Type(unescape(rest, line)?));
    }
    if let Some(rest) = raw.strip_prefix("paste ") {
        return Ok(Action::Paste(unescape(rest, line)?));
    }
    if let Some(rest) = raw.strip_prefix("clip ") {
        return Ok(Action::ClipboardReply(unescape(rest, line)?));
    }
    if let Some(rest) = raw.strip_prefix("resize ") {
        return parse_resize(rest, line);
    }
    if let Some(rest) = raw.strip_prefix("key ") {
        return parse_key(rest, line).map(Action::Key);
    }

    let keyword = raw.split(' ').next().unwrap_or(raw);
    Err(ScriptError::UnknownKeyword {
        line,
        keyword: keyword.to_string(),
    })
}

fn parse_resize(rest: &str, line: usize) -> Result<Action, ScriptError> {
    let malformed = || ScriptError::MalformedLine {
        line,
        reason: "expected `resize <width> <height>`".to_string(),
    };
    let mut parts = rest.split(' ');
    let w_str = parts.next().ok_or_else(malformed)?;
    let h_str = parts.next().ok_or_else(malformed)?;
    if parts.next().is_some() {
        return Err(malformed());
    }
    let num = |field: &str, v: &str| -> Result<u16, ScriptError> {
        v.parse().map_err(|_| ScriptError::InvalidNumber {
            line,
            reason: format!("invalid {field} {v:?}"),
        })
    };
    Ok(Action::Resize(
        num("resize width", w_str)?,
        num("resize height", h_str)?,
    ))
}

/// Splits a `key` line's remainder into code and mods fields by taking the
/// LAST 4 characters as mods and the character before that as the
/// separator — never by a generic whitespace split (module docs).
fn parse_key(rest: &str, line: usize) -> Result<KeyInput, ScriptError> {
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

/// Mirrors `encode_code`'s spellings; the `_` arm is the only place a
/// `char:` payload is accepted.
fn parse_code(s: &str, line: usize) -> Result<KeyCode, ScriptError> {
    let code = match s {
        "enter" => KeyCode::Enter,
        "backspace" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    /// Fails loudly on an unexpected `Err` without an infallible-unwrap call
    /// (keeps this whole file free of that family of call, tests included).
    fn must_decode(text: &str) -> (String, Vec<Action>) {
        let result = decode(text);
        assert!(result.is_ok(), "decode({text:?}) failed: {result:?}");
        result.unwrap_or_else(|_| (String::new(), Vec::new()))
    }

    fn key(code: KeyCode, mods: Mods) -> Action {
        Action::Key(KeyInput { code, mods })
    }

    fn mods(shift: bool, alt: bool, ctrl: bool, sup: bool) -> Mods {
        Mods {
            shift,
            alt,
            ctrl,
            sup,
        }
    }

    #[test]
    fn round_trips_every_action_variant() {
        let content = "hello\nworld";
        let actions = vec![
            key(KeyCode::Char('a'), Mods::NONE),
            key(KeyCode::Left, mods(true, false, false, false)),
            key(KeyCode::Char(' '), mods(false, false, false, true)),
            key(KeyCode::Char('😀'), mods(true, true, true, true)),
            Action::Type("some prose".to_string()),
            Action::Paste("line1\r\nline2".to_string()),
            Action::Resize(80, 24),
            Action::ClipboardReply("clipboard text".to_string()),
            Action::ConfirmTimeout,
            Action::Deliver,
            Action::FailNextSave,
            Action::DirLoaded {
                entries: vec![
                    DirEntry {
                        name: "sub dir".to_string(), // a literal space in the name
                        is_dir: true,
                    },
                    DirEntry {
                        name: "a.md".to_string(),
                        is_dir: false,
                    },
                ],
                cause: DirCause::Nav,
                generation: 7,
            },
            Action::DirLoaded {
                entries: Vec::new(),
                cause: DirCause::Refresh,
                generation: 0,
            },
        ];

        let encoded = encode(content, &actions);
        assert_eq!(must_decode(&encoded), (content.to_string(), actions));
    }

    #[test]
    fn escapes_newline_carriage_return_tab_quote_nul_and_emoji() {
        let cases: &[(char, &str)] = &[
            ('\n', "\\n"),
            ('\r', "\\r"),
            ('\t', "\\t"),
            ('"', "\\\""),
            ('\0', "\\u{0}"),
            ('😀', "\\u{1f600}"),
        ];
        for &(ch, want_fragment) in cases {
            let actions = vec![Action::Type(format!("x{ch}y"))];
            let encoded = encode("", &actions);
            assert!(
                encoded.contains(want_fragment),
                "encoding {ch:?} should contain {want_fragment:?}, got {encoded:?}"
            );
            assert_eq!(must_decode(&encoded).1, actions);
        }
    }

    #[test]
    fn rejects_a_malformed_line_with_a_typed_error() {
        let err = decode("content hi\nbogus-keyword-here\n").unwrap_err();
        assert!(
            matches!(err, ScriptError::UnknownKeyword { ref keyword, .. }
            if keyword == "bogus-keyword-here")
        );

        let err = decode("no content line here\n").unwrap_err();
        assert!(matches!(err, ScriptError::MalformedLine { .. }));

        let err = decode("# only a comment\n\n").unwrap_err();
        assert_eq!(err, ScriptError::MissingContentLine);

        let err = decode("content hi\nkey --\n").unwrap_err();
        assert!(matches!(err, ScriptError::MalformedLine { .. }));

        let err = decode("content hi\nkey char:a xxxx\n").unwrap_err();
        assert!(matches!(err, ScriptError::InvalidMods { .. }));

        let err = decode("content hi\nresize wide tall\n").unwrap_err();
        assert!(matches!(err, ScriptError::InvalidNumber { .. }));
    }

    #[test]
    fn skips_comments_and_blank_lines() {
        let text = "# a leading comment\n\ncontent hi\n\n# a comment between actions\ntype x\n";
        let (content, actions) = must_decode(text);
        assert_eq!(content, "hi");
        assert_eq!(actions, vec![Action::Type("x".to_string())]);
    }
}
