//! The encode half of the script codec (module docs). Always infallible —
//! `Action`/`KeyCode`/`Mods` are already well-formed Rust values, so there is
//! nothing here that can fail the way `decode` can.

use crate::action::{Action, HighlightVersion};
use crate::driver::DOC_PATH;
use rune_tui::keymap::{KeyCode, Mods};
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
        Action::Highlight { version, spans } => {
            out.push_str("highlight ");
            out.push_str(encode_highlight_version(*version));
            out.push(' ');
            out.push_str(&spans.len().to_string());
            out.push('\n');
            for (start, end, scope) in spans {
                out.push_str(&format!("highlight-span {start} {end} {scope}\n"));
            }
        }
    }
}

fn encode_highlight_version(version: HighlightVersion) -> &'static str {
    match version {
        HighlightVersion::Live => "live",
        HighlightVersion::Stale => "stale",
        HighlightVersion::Future => "future",
    }
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
