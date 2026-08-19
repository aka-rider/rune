//! The encode half of the script codec (module docs). Always infallible —
//! `Action`/`KeyCode`/`Mods` are already well-formed Rust values, so there is
//! nothing here that can fail the way `decode` can.

use std::fmt::Write as _;

use super::keyword::{self, Keyword};
use crate::action::{Action, HighlightVersion, PaletteGenClaim};
use crate::driver::DOC_PATH;
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::DirCause;

/// Encodes `(path, content, actions)` as script text, one action per line.
/// The `path` line (plan WP7.S2) is emitted only when `path != DOC_PATH` —
/// every script written before a session carried a path (including the
/// checked-in `repros/tripwire-clean.rune`) implicitly meant the default,
/// so a session that still opens the default path keeps encoding to
/// exactly that same terse form.
pub fn encode(path: &str, content: &str, actions: &[Action]) -> String {
    let mut out = String::new();
    out.push_str("content ");
    out.push_str(&escape(content));
    out.push('\n');
    if path != DOC_PATH {
        out.push_str("path ");
        out.push_str(&escape(path));
        out.push('\n');
    }
    for action in actions {
        encode_action(&mut out, action);
    }
    out
}

fn encode_action(out: &mut String, action: &Action) {
    out.push_str(Keyword::for_action(action).as_str());
    match action {
        Action::Key(k) => {
            out.push(' ');
            out.push_str(&encode_code(k.code));
            out.push(' ');
            out.push_str(&encode_mods(k.mods));
            out.push('\n');
        }
        Action::Mouse(m) => {
            let kind = encode_mouse_kind(m.kind);
            let mods = encode_mouse_mods(*m);
            let _ = writeln!(out, " {kind} {} {} {mods}", m.column, m.row);
        }
        Action::Type(s) | Action::Paste(s) | Action::ClipboardReply(s) => {
            out.push(' ');
            out.push_str(&escape(s));
            out.push('\n');
        }
        Action::OpenFileSearch
        | Action::ConfirmTimeout
        | Action::Deliver
        | Action::FailNextSave => {
            out.push('\n');
        }
        Action::Resize(w, h) => {
            let _ = writeln!(out, " {w} {h}");
        }
        Action::StaleConfirmTimeout(generation) => {
            out.push(' ');
            out.push_str(&generation.to_string());
            out.push('\n');
        }
        Action::DirLoaded {
            entries,
            cause,
            generation,
        } => {
            out.push(' ');
            out.push_str(encode_dir_cause(*cause));
            out.push(' ');
            out.push_str(&generation.to_string());
            out.push('\n');
            for entry in entries {
                out.push_str(keyword::DIRLOADED_ENTRY);
                out.push(' ');
                out.push(encode_file_kind(entry.kind));
                out.push(' ');
                out.push(encode_link(entry.link));
                out.push(' ');
                out.push_str(&escape(&entry.name));
                out.push('\n');
            }
        }
        Action::Highlight { version, spans } => {
            out.push(' ');
            out.push_str(encode_highlight_version(*version));
            out.push(' ');
            out.push_str(&spans.len().to_string());
            out.push('\n');
            for (start, end, scope) in spans {
                out.push_str(keyword::HIGHLIGHT_SPAN);
                let _ = writeln!(out, " {start} {end} {scope}");
            }
        }
        Action::DivergeDisk | Action::DeliverDb | Action::DeliverDbAll => out.push('\n'),
        Action::HighlightTree {
            version,
            fixture,
            base,
        } => {
            out.push(' ');
            out.push_str(encode_highlight_version(*version));
            out.push(' ');
            out.push_str(&fixture.to_string());
            out.push(' ');
            out.push_str(&base.to_string());
            out.push('\n');
        }
        Action::AdvanceClock(millis) => {
            out.push(' ');
            out.push_str(&millis.to_string());
            out.push('\n');
        }
        Action::PaletteRecentsLoaded {
            generation,
            ok,
            names,
        } => {
            out.push(' ');
            out.push_str(&encode_palette_gen_claim(generation));
            out.push(' ');
            out.push_str(if *ok { "ok" } else { "err" });
            out.push(' ');
            out.push_str(&names.len().to_string());
            out.push('\n');
            for name in names {
                out.push_str(keyword::PALETTE_RECENTS_NAME);
                out.push(' ');
                out.push_str(&escape(name));
                out.push('\n');
            }
        }
    }
}

fn encode_palette_gen_claim(claim: &PaletteGenClaim) -> String {
    match claim {
        PaletteGenClaim::Live => "live".to_string(),
        PaletteGenClaim::Stale(raw) => format!("stale:{raw}"),
    }
}

fn encode_highlight_version(version: HighlightVersion) -> &'static str {
    match version {
        HighlightVersion::Live => "live",
        HighlightVersion::Stale => "stale",
        HighlightVersion::Future => "future",
    }
}

/// Exhaustive on `MouseKind`/`MouseButton` (compiler-checked the same way
/// `encode_code` is against `decode`'s mirror match).
fn encode_mouse_kind(kind: MouseKind) -> String {
    let button = |b: MouseButton| match b {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
    };
    match kind {
        MouseKind::Down(b) => format!("down:{}", button(b)),
        MouseKind::Up(b) => format!("up:{}", button(b)),
        MouseKind::Drag(b) => format!("drag:{}", button(b)),
        MouseKind::ScrollUp => "scroll-up".into(),
        MouseKind::ScrollDown => "scroll-down".into(),
    }
}

fn encode_mouse_mods(m: MouseInput) -> String {
    let mut s = String::with_capacity(3);
    s.push(if m.shift { 's' } else { '-' });
    s.push(if m.alt { 'a' } else { '-' });
    s.push(if m.ctrl { 'c' } else { '-' });
    s
}

fn encode_dir_cause(cause: DirCause) -> &'static str {
    match cause {
        DirCause::Nav => "nav",
        DirCause::Refresh => "refresh",
    }
}

/// Exhaustive on `KeyCode` (compiler-checked: adding a variant breaks this
/// build until it's handled here and in `decode::parse_code`'s mirror
/// match).
fn encode_code(code: KeyCode) -> String {
    match code {
        KeyCode::Char(c) => format!("char:{}", escape_char(c)),
        KeyCode::Enter => "enter".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Tab => "tab".into(),
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
    s.chars().flat_map(char::escape_default).collect()
}

fn encode_file_kind(kind: rune_vfs::FileKind) -> char {
    match kind {
        rune_vfs::FileKind::File => 'f',
        rune_vfs::FileKind::Dir => 'd',
        rune_vfs::FileKind::Other => 'o',
    }
}

fn encode_link(link: rune_vfs::Link) -> char {
    match link {
        rune_vfs::Link::No => 'n',
        rune_vfs::Link::To => 't',
        rune_vfs::Link::Broken => 'b',
    }
}
