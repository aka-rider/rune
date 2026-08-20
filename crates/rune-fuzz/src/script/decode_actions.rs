use std::iter::Peekable;
use std::path::PathBuf;

use super::{strip_token, unescape};
use crate::action::{Action, HighlightVersion, PaletteGenClaim};
use crate::script::ScriptError;
use crate::script::keyword;
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::DirCause;
use rune_vfs::DirEntry;

/// Parses a `dirloaded <cause>` line plus its `dirloaded-entry` continuation
/// lines (module docs) — the one multi-line action in this grammar, so this
/// is the only parser that needs to look ahead in `lines`.
pub(super) fn parse_dir_loaded<'a>(
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
        reason: "expected `dirloaded-entry <f|d|o> <n|t|b> <name>`".to_string(),
    };
    let mut parts = rest.splitn(3, ' ');
    let kind_flag = parts.next().ok_or_else(malformed)?;
    let link_flag = parts.next().ok_or_else(malformed)?;
    let name_field = parts.next().ok_or_else(malformed)?;

    let kind = match kind_flag {
        "f" => rune_vfs::FileKind::File,
        "d" => rune_vfs::FileKind::Dir,
        "o" => rune_vfs::FileKind::Other,
        _ => return Err(malformed()),
    };

    let link = match link_flag {
        "n" => rune_vfs::Link::No,
        "t" => rune_vfs::Link::To,
        "b" => rune_vfs::Link::Broken,
        _ => return Err(malformed()),
    };

    let name = unescape(name_field, line)?;
    let path = PathBuf::from(&name);
    Ok(DirEntry {
        name,
        path,
        kind,
        link,
    })
}

/// Parses a `highlight <live|stale|future> <n>` line plus its `n`
/// `highlight-span <start> <end> <scope>` continuation lines (plan
/// WP7.S5) — the count is known up front (unlike `dirloaded-entry`'s
/// peek-until-mismatch loop), so this consumes exactly `n` lines, erroring
/// if one doesn't match the expected prefix or the input runs out early.
pub(super) fn parse_highlight<'a>(
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
pub(super) fn parse_highlight_tree(rest: &str, line: usize) -> Result<Action, ScriptError> {
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

pub(super) fn parse_palette_recents<'a>(
    rest: &str,
    line: usize,
    lines: &mut Peekable<impl Iterator<Item = (usize, &'a str)>>,
) -> Result<Action, ScriptError> {
    let malformed = || ScriptError::MalformedLine {
        line,
        reason: "expected `palette-recents <live|stale:<u32>> <ok|err> <n>`".to_string(),
    };
    let mut parts = rest.trim().splitn(3, ' ');
    let gen_str = parts.next().ok_or_else(malformed)?;
    let generation = if gen_str == "live" {
        PaletteGenClaim::Live
    } else if let Some(raw) = gen_str.strip_prefix("stale:") {
        let raw: u32 = raw.parse().map_err(|_| ScriptError::InvalidNumber {
            line,
            reason: "expected a u32 generation".to_string(),
        })?;
        PaletteGenClaim::Stale(raw)
    } else {
        return Err(malformed());
    };
    let ok_str = parts.next().ok_or_else(malformed)?;
    let ok = match ok_str {
        "ok" => true,
        "err" => false,
        _ => return Err(malformed()),
    };
    let n: usize = parts
        .next()
        .ok_or_else(malformed)?
        .trim()
        .parse()
        .map_err(|_| ScriptError::InvalidNumber {
            line,
            reason: "expected a usize name count".to_string(),
        })?;

    let mut names = Vec::with_capacity(n);
    for _ in 0..n {
        let Some((next_idx, next_raw)) = lines.next() else {
            return Err(ScriptError::MalformedLine {
                line,
                reason: format!("expected {n} palette-recents-name lines, ran out of input"),
            });
        };
        let entry_line = next_idx + 1;
        let entry_rest = strip_token(next_raw, keyword::PALETTE_RECENTS_NAME).ok_or_else(|| {
            ScriptError::MalformedLine {
                line: entry_line,
                reason: "expected `palette-recents-name <name>`".to_string(),
            }
        })?;
        names.push(unescape(entry_rest, entry_line)?);
    }

    Ok(Action::PaletteRecentsLoaded {
        generation,
        ok,
        names,
    })
}

/// Parses a single-line `mouse <kind> <col> <row> <mods>` action — every
/// field is space-free by construction (no escaped payload), so a plain
/// split suffices, unlike `parse_key`'s last-4-chars mods recovery. `<mods>`
/// is a fixed 3-char field (shift, alt, ctrl — a mouse event carries no
/// `sup`), each `-` or its initial letter, mirroring `key`'s convention.
pub(super) fn parse_mouse(rest: &str, line: usize) -> Result<Action, ScriptError> {
    let malformed = || ScriptError::MalformedLine {
        line,
        reason: "expected `mouse <kind> <col> <row> <mods>`".to_string(),
    };
    let mut parts = rest.split(' ');
    let kind_str = parts.next().ok_or_else(malformed)?;
    let col_str = parts.next().ok_or_else(malformed)?;
    let row_str = parts.next().ok_or_else(malformed)?;
    let mods_str = parts.next().ok_or_else(malformed)?;
    if parts.next().is_some() {
        return Err(malformed());
    }

    let kind = parse_mouse_kind(kind_str, line)?;
    let num = |field: &str, v: &str| -> Result<u16, ScriptError> {
        v.parse().map_err(|_| ScriptError::InvalidNumber {
            line,
            reason: format!("invalid {field} {v:?}"),
        })
    };
    let column = num("mouse col", col_str)?;
    let row = num("mouse row", row_str)?;

    let invalid_mods = || ScriptError::InvalidMods {
        line,
        mods: mods_str.to_string(),
    };
    let chars: Vec<char> = mods_str.chars().collect();
    if chars.len() != 3 {
        return Err(invalid_mods());
    }
    let flag = |idx: usize, on: char| match chars.get(idx) {
        Some('-') => Ok(false),
        Some(&c) if c == on => Ok(true),
        _ => Err(invalid_mods()),
    };
    Ok(Action::Mouse(MouseInput {
        kind,
        column,
        row,
        shift: flag(0, 's')?,
        alt: flag(1, 'a')?,
        ctrl: flag(2, 'c')?,
    }))
}

/// Mirrors `encode::encode_mouse_kind`'s spellings.
fn parse_mouse_kind(s: &str, line: usize) -> Result<MouseKind, ScriptError> {
    let malformed = || ScriptError::MalformedLine {
        line,
        reason: format!("unknown mouse kind {s:?}"),
    };
    if s == "scroll-up" {
        return Ok(MouseKind::ScrollUp);
    }
    if s == "scroll-down" {
        return Ok(MouseKind::ScrollDown);
    }
    let (verb, button_str) = s.split_once(':').ok_or_else(malformed)?;
    let button = match button_str {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        _ => return Err(malformed()),
    };
    match verb {
        "down" => Ok(MouseKind::Down(button)),
        "up" => Ok(MouseKind::Up(button)),
        "drag" => Ok(MouseKind::Drag(button)),
        _ => Err(malformed()),
    }
}

pub(super) fn parse_resize(rest: &str, line: usize) -> Result<Action, ScriptError> {
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
