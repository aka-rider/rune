//! The decode half of the script codec (module docs). Every fallible step
//! returns a `ScriptError`, mirroring `rune_core::buffer::BufferError`'s
//! idiom (G17) — no `unwrap`/`expect`/`panic!`/unchecked indexing anywhere
//! in this file.

use super::ScriptError;
use super::decode_key::parse_key;
use super::keyword::Keyword;
use crate::action::Action;
use crate::driver::DOC_PATH;

#[path = "decode_actions.rs"]
mod decode_actions;

use decode_actions::{
    parse_dir_loaded, parse_highlight, parse_highlight_tree, parse_mouse, parse_palette_recents,
    parse_resize,
};

fn strip_token<'a>(raw: &'a str, token: &str) -> Option<&'a str> {
    raw.strip_prefix(token)?.strip_prefix(' ')
}

fn is_token(raw: &str, token: &str) -> bool {
    raw == token
}

/// Hand-written unescape, the inverse of `encode::escape`/`escape_char`.
/// Handles exactly `\n \r \t \\ \' \" \0 \u{...}`. `line` attributes an
/// error to the right source line.
pub(super) fn unescape(s: &str, line: usize) -> Result<String, ScriptError> {
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
/// are comments, skipped wherever they appear. Returns `(path, content,
/// actions)` — `path` (plan WP7.S2) is an OPTIONAL line permitted only
/// immediately after `content` (before any action line); its absence
/// defaults to `DOC_PATH`, so every script written before sessions carried
/// a path — including the checked-in `repros/tripwire-clean.rune` — still
/// decodes unchanged.
pub fn decode(text: &str) -> Result<(String, String, Vec<Action>), ScriptError> {
    let mut content: Option<String> = None;
    let mut path: Option<String> = None;
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

        if path.is_none()
            && actions.is_empty()
            && let Some(rest) = raw.strip_prefix("path ")
        {
            path = Some(unescape(rest, line)?);
            continue;
        }

        if let Some(rest) = strip_token(raw, Keyword::DirLoaded.as_str()) {
            actions.push(parse_dir_loaded(rest, line, &mut lines)?);
            continue;
        }

        if let Some(rest) = strip_token(raw, Keyword::Highlight.as_str()) {
            actions.push(parse_highlight(rest, line, &mut lines)?);
            continue;
        }

        if let Some(rest) = strip_token(raw, Keyword::PaletteRecentsLoaded.as_str()) {
            actions.push(parse_palette_recents(rest, line, &mut lines)?);
            continue;
        }

        actions.push(parse_action_line(raw, line)?);
    }

    let content = content.ok_or(ScriptError::MissingContentLine)?;
    let path = path.unwrap_or_else(|| DOC_PATH.to_string());
    Ok((path, content, actions))
}

fn parse_action_line(raw: &str, line: usize) -> Result<Action, ScriptError> {
    if is_token(raw, Keyword::ConfirmTimeout.as_str()) {
        return Ok(Action::ConfirmTimeout);
    }
    if let Some(rest) = strip_token(raw, Keyword::StaleConfirmTimeout.as_str()) {
        let generation: u32 = rest
            .trim()
            .parse()
            .map_err(|_| ScriptError::InvalidNumber {
                line,
                reason: "expected a u32 generation".to_string(),
            })?;
        return Ok(Action::StaleConfirmTimeout(generation));
    }
    if is_token(raw, Keyword::Deliver.as_str()) {
        return Ok(Action::Deliver);
    }
    if is_token(raw, Keyword::FailNextSave.as_str()) {
        return Ok(Action::FailNextSave);
    }
    if is_token(raw, Keyword::DivergeDisk.as_str()) {
        return Ok(Action::DivergeDisk);
    }
    if is_token(raw, Keyword::DeliverDbAll.as_str()) {
        return Ok(Action::DeliverDbAll);
    }
    if is_token(raw, Keyword::DeliverDb.as_str()) {
        return Ok(Action::DeliverDb);
    }
    if is_token(raw, Keyword::OpenFileSearch.as_str()) {
        return Ok(Action::OpenFileSearch);
    }
    if let Some(rest) = strip_token(raw, Keyword::Type.as_str()) {
        let text = unescape(rest, line)?;
        // Reject any control char other than `\n` (CODE-REVIEW.md rune-fuzz
        // finding 4): `Action::Type` can only ever deliver these one `char`
        // at a time as a `Msg::Key`, unlike `Action::Paste`, which carries
        // arbitrary bytes verbatim.
        if let Some(ch) = text.chars().find(|&ch| ch != '\n' && ch.is_control()) {
            return Err(ScriptError::UndeliverableTypeChar { line, ch });
        }
        return Ok(Action::Type(text));
    }
    if let Some(rest) = strip_token(raw, Keyword::Paste.as_str()) {
        return Ok(Action::Paste(unescape(rest, line)?));
    }
    if let Some(rest) = strip_token(raw, Keyword::ClipboardReply.as_str()) {
        return Ok(Action::ClipboardReply(unescape(rest, line)?));
    }
    if let Some(rest) = strip_token(raw, Keyword::Resize.as_str()) {
        return parse_resize(rest, line);
    }
    if let Some(rest) = strip_token(raw, Keyword::Key.as_str()) {
        return parse_key(rest, line).map(Action::Key);
    }
    if let Some(rest) = strip_token(raw, Keyword::Mouse.as_str()) {
        return parse_mouse(rest, line);
    }
    if let Some(rest) = strip_token(raw, Keyword::HighlightTree.as_str()) {
        return parse_highlight_tree(rest, line);
    }
    if let Some(rest) = strip_token(raw, Keyword::AdvanceClock.as_str()) {
        let millis: u64 = rest
            .trim()
            .parse()
            .map_err(|_| ScriptError::InvalidNumber {
                line,
                reason: "expected a u64 millisecond count".to_string(),
            })?;
        return Ok(Action::AdvanceClock(millis));
    }
    if let Some(rest) = strip_token(raw, Keyword::InstallDiffLeft.as_str()) {
        let seed_index: u8 = rest
            .trim()
            .parse()
            .map_err(|_| ScriptError::InvalidNumber {
                line,
                reason: "expected a u8 seed index".to_string(),
            })?;
        return Ok(Action::InstallDiffLeft { seed_index });
    }

    let keyword = raw.split(' ').next().unwrap_or(raw);
    Err(ScriptError::UnknownKeyword {
        line,
        keyword: keyword.to_string(),
    })
}

