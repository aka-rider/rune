//! The decode half of the script codec (module docs). Every fallible step
//! returns a `ScriptError`, mirroring `rune_core::buffer::BufferError`'s
//! idiom (G17) — no `unwrap`/`expect`/`panic!`/unchecked indexing anywhere
//! in this file.

use std::iter::Peekable;
use std::path::PathBuf;

use super::ScriptError;
use super::decode_key::parse_key;
use super::keyword::{self, Keyword};
use crate::action::{Action, HighlightVersion};
use crate::driver::DOC_PATH;
use rune_tui::runtime::DirCause;
use rune_vfs::DirEntry;

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

        actions.push(parse_action_line(raw, line)?);
    }

    let content = content.ok_or(ScriptError::MissingContentLine)?;
    let path = path.unwrap_or_else(|| DOC_PATH.to_string());
    Ok((path, content, actions))
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
        let Some(entry_rest) = strip_token(next_raw, keyword::DIRLOADED_ENTRY) else {
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
    let kind = match flag {
        "d" => rune_vfs::FileKind::Dir,
        "f" => rune_vfs::FileKind::File,
        _ => return Err(malformed()),
    };
    // WP13.S1: the script codec is text-only (its whole point is a
    // human-readable, round-trippable session log), so `path` is derived
    // straight from the decoded `name` — never lossy, since the codec
    // never carries anything but valid Unicode.
    let name = unescape(name_field, line)?;
    let path = PathBuf::from(&name);
    Ok(DirEntry { name, path, kind })
}

/// Parses a `highlight <live|stale|future> <n>` line plus its `n`
/// `highlight-span <start> <end> <scope>` continuation lines (plan
/// WP7.S5) — the count is known up front (unlike `dirloaded-entry`'s
/// peek-until-mismatch loop), so this consumes exactly `n` lines, erroring
/// if one doesn't match the expected prefix or the input runs out early.
fn parse_highlight<'a>(
    rest: &str,
    line: usize,
    lines: &mut Peekable<impl Iterator<Item = (usize, &'a str)>>,
) -> Result<Action, ScriptError> {
    let malformed = || ScriptError::MalformedLine {
        line,
        reason: "expected `highlight <live|stale|future> <n>`".to_string(),
    };
    let mut parts = rest.trim().splitn(2, ' ');
    let version = match parts.next().ok_or_else(malformed)? {
        "live" => HighlightVersion::Live,
        "stale" => HighlightVersion::Stale,
        "future" => HighlightVersion::Future,
        other => {
            return Err(ScriptError::MalformedLine {
                line,
                reason: format!("unknown highlight version {other:?}"),
            });
        }
    };
    let n: usize = parts
        .next()
        .ok_or_else(malformed)?
        .trim()
        .parse()
        .map_err(|_| ScriptError::InvalidNumber {
            line,
            reason: "expected a usize span count".to_string(),
        })?;

    let mut spans = Vec::with_capacity(n);
    for _ in 0..n {
        let Some((next_idx, next_raw)) = lines.next() else {
            return Err(ScriptError::MalformedLine {
                line,
                reason: format!("expected {n} highlight-span lines, ran out of input"),
            });
        };
        let entry_line = next_idx + 1;
        let entry_rest = strip_token(next_raw, keyword::HIGHLIGHT_SPAN).ok_or_else(|| {
            ScriptError::MalformedLine {
                line: entry_line,
                reason: "expected `highlight-span <start> <end> <scope>`".to_string(),
            }
        })?;
        spans.push(parse_highlight_span(entry_rest, entry_line)?);
    }

    Ok(Action::Highlight { version, spans })
}

fn parse_highlight_span(rest: &str, line: usize) -> Result<(usize, usize, u16), ScriptError> {
    let malformed = || ScriptError::MalformedLine {
        line,
        reason: "expected `highlight-span <start> <end> <scope>`".to_string(),
    };
    let mut parts = rest.split(' ');
    let start_str = parts.next().ok_or_else(malformed)?;
    let end_str = parts.next().ok_or_else(malformed)?;
    let scope_str = parts.next().ok_or_else(malformed)?;
    if parts.next().is_some() {
        return Err(malformed());
    }
    let num = |field: &str, v: &str| -> Result<usize, ScriptError> {
        v.parse().map_err(|_| ScriptError::InvalidNumber {
            line,
            reason: format!("invalid {field} {v:?}"),
        })
    };
    let start = num("highlight-span start", start_str)?;
    let end = num("highlight-span end", end_str)?;
    let scope: u16 = scope_str.parse().map_err(|_| ScriptError::InvalidNumber {
        line,
        reason: format!("invalid highlight-span scope {scope_str:?}"),
    })?;
    Ok((start, end, scope))
}

/// Parses a single-line `highlight-tree <live|stale|future> <fixture> <base>`
/// action — unlike `highlight`, this has no continuation lines, since the
/// tree channel carries no per-delivery span list.
fn parse_highlight_tree(rest: &str, line: usize) -> Result<Action, ScriptError> {
    let malformed = || ScriptError::MalformedLine {
        line,
        reason: "expected `highlight-tree <live|stale|future> <fixture> <base>`".to_string(),
    };
    let mut parts = rest.split(' ');
    let version_str = parts.next().ok_or_else(malformed)?;
    let fixture_str = parts.next().ok_or_else(malformed)?;
    let base_str = parts.next().ok_or_else(malformed)?;
    if parts.next().is_some() {
        return Err(malformed());
    }
    let version = match version_str {
        "live" => HighlightVersion::Live,
        "stale" => HighlightVersion::Stale,
        "future" => HighlightVersion::Future,
        other => {
            return Err(ScriptError::MalformedLine {
                line,
                reason: format!("unknown highlight-tree version {other:?}"),
            });
        }
    };
    let fixture: u8 = fixture_str
        .parse()
        .map_err(|_| ScriptError::InvalidNumber {
            line,
            reason: format!("invalid highlight-tree fixture {fixture_str:?}"),
        })?;
    let base: usize = base_str.parse().map_err(|_| ScriptError::InvalidNumber {
        line,
        reason: format!("invalid highlight-tree base {base_str:?}"),
    })?;
    Ok(Action::HighlightTree {
        version,
        fixture,
        base,
    })
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
