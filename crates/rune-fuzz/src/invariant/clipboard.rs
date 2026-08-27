//! `PASTE-VERBATIM`/`CLIP-OSC52` — the two clipboard-path byte-verbatim
//! invariants.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use rune_core::buffer::Buffer;
use rune_tui::focus::FocusTarget;
use rune_tui::keymap::Command;
use rune_tui::pane::Pane;
use rune_tui::runtime::PasteTarget;

use super::{Violation, trunc};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// `PASTE-VERBATIM` — dispatches on `prev.focus_target`, the same
/// `FocusTarget` the bracketed paste router itself reads,
/// so a `Msg::Paste` step is checked against exactly where production sent
/// it, never a parallel re-derivation. `Msg::ClipboardRead` carries its own
/// `target`, captured at request time, so that arm checks against it
/// directly instead.
pub fn paste_verbatim(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    match &ctx.msg {
        MsgTag::Paste(text) => bracketed_paste_violation(prev, next, text),
        MsgTag::ClipboardRead { text, target } if *target == PasteTarget::Document(prev.active) => {
            document_paste_violation(prev, next, text)
        }
        _ => None,
    }
}

fn bracketed_paste_violation(prev: &Snapshot, next: &Snapshot, text: &str) -> Option<Violation> {
    if text.is_empty() {
        return None;
    }
    match prev.focus_target {
        FocusTarget::SearchField => search_paste_violation(prev, next, text),
        FocusTarget::FileSearch => filesearch_paste_violation(prev, next, text),
        FocusTarget::Palette => palette_paste_violation(prev, next, text),
        FocusTarget::Title => title_paste_violation(prev, next, text),
        FocusTarget::Editor | FocusTarget::ReplaceField => {
            document_paste_violation(prev, next, text)
        }
        FocusTarget::Explorer | FocusTarget::Tabs | FocusTarget::Messages => {
            chrome_pane_paste_refused_violation(prev, next)
        }
    }
}

fn chrome_pane_paste_refused_violation(prev: &Snapshot, next: &Snapshot) -> Option<Violation> {
    if next.content == prev.content {
        return None;
    }
    Some(Violation::new(
        "PASTE-VERBATIM",
        format!(
            "paste with a chrome pane focused must refuse, but the document changed: {:?} -> {:?}",
            trunc(&prev.content, 120),
            trunc(&next.content, 120)
        ),
    ))
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

fn strip_control(text: &str) -> String {
    first_line(text)
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

fn append_violation(
    field: &str,
    prev: &Option<String>,
    next: &Option<String>,
    sanitized: &str,
) -> Option<Violation> {
    let prev_text = prev.clone().unwrap_or_default();
    let expected = format!("{prev_text}{sanitized}");
    let actual = next.clone().unwrap_or_default();
    if actual == expected {
        return None;
    }
    Some(Violation::new(
        "PASTE-VERBATIM",
        format!(
            "pasted text not appended to the {field}: expected={:?} got={:?}",
            trunc(&expected, 120),
            trunc(&actual, 120)
        ),
    ))
}

fn search_paste_violation(prev: &Snapshot, next: &Snapshot, text: &str) -> Option<Violation> {
    let sanitized = strip_control(text);
    append_violation(
        "search field",
        &prev.search_draft,
        &next.search_draft,
        &sanitized,
    )
}

fn filesearch_paste_violation(prev: &Snapshot, next: &Snapshot, text: &str) -> Option<Violation> {
    let sanitized = strip_control(text);
    append_violation(
        "file-search query",
        &prev.filesearch_query,
        &next.filesearch_query,
        &sanitized,
    )
}

fn palette_paste_violation(prev: &Snapshot, next: &Snapshot, text: &str) -> Option<Violation> {
    let sanitized = strip_control(text);
    append_violation(
        "command palette query",
        &prev.palette_query,
        &next.palette_query,
        &sanitized,
    )
}

fn title_paste_violation(prev: &Snapshot, next: &Snapshot, text: &str) -> Option<Violation> {
    let sanitized: String = first_line(text)
        .chars()
        .filter(|&c| rune_tui::title::is_name_char(c))
        .collect();

    if sanitized.is_empty() {
        if next.title_text == prev.title_text {
            return None;
        }
        return Some(Violation::new(
            "PASTE-VERBATIM",
            format!(
                "title field changed despite an empty sanitized paste: {:?} -> {:?}",
                trunc(&prev.title_text, 120),
                trunc(&next.title_text, 120)
            ),
        ));
    }

    let cursor = prev.title_cursor;
    let (raw_start, raw_end) = if cursor.has_selection() {
        let (s, e) = cursor.selection_range();
        (s.get(), e.get())
    } else {
        (cursor.position.get(), cursor.position.get())
    };
    let window = &prev.title_window;
    let start = prev
        .title_text
        .floor_char_boundary(raw_start.clamp(window.start, window.end));
    let end = prev
        .title_text
        .floor_char_boundary(raw_end.clamp(window.start, window.end));
    if start > end || end > prev.title_text.len() {
        return None; // a malformed field cursor is CUR-BOUNDS's job to report
    }

    let mut expected = String::with_capacity(prev.title_text.len() + sanitized.len());
    expected.push_str(&prev.title_text[..start]);
    expected.push_str(&sanitized);
    expected.push_str(&prev.title_text[end..]);

    if next.title_text == expected {
        return None;
    }
    Some(Violation::new(
        "PASTE-VERBATIM",
        format!(
            "pasted text not inserted into the title field at [{start}, {end}): expected={:?} got={:?}",
            trunc(&expected, 120),
            trunc(&next.title_text, 120)
        ),
    ))
}

fn document_paste_violation(prev: &Snapshot, next: &Snapshot, text: &str) -> Option<Violation> {
    if text.is_empty() || prev.read_only != rune_tui::document::ReadOnly::No {
        return None;
    }
    let [cursor] = prev.cursors.as_slice() else {
        return None;
    };
    let (start, end) = cursor.selection_range();
    let (start, end) = (start.get(), end.get());
    if end > prev.content.len()
        || !prev.content.is_char_boundary(start)
        || !prev.content.is_char_boundary(end)
    {
        return None; // a malformed cursor is CUR-BOUNDS's job to report
    }

    let mut expected = String::with_capacity(prev.content.len() + text.len());
    expected.push_str(&prev.content[..start]);
    expected.push_str(text);
    expected.push_str(&prev.content[end..]);

    if next.content == expected {
        return None;
    }
    Some(Violation::new(
        "PASTE-VERBATIM",
        format!(
            "pasted text not substituted verbatim at [{start}, {end}): expected={:?} got={:?}",
            trunc(&expected, 120),
            trunc(&next.content, 120)
        ),
    ))
}

/// Decodes an OSC 52 "set clipboard" sequence
/// (`\x1b]52;c;<base64>\x07`, `rune_tui::clipboard::osc52_copy`'s exact
/// wire format) back to its raw payload, or `None` if `bytes` isn't one.
fn decode_osc52(bytes: &[u8]) -> Option<Vec<u8>> {
    let rest = bytes.strip_prefix(b"\x1b]52;c;")?;
    let rest = rest.strip_suffix(b"\x07")?;
    STANDARD.decode(rest).ok()
}

/// `CLIP-OSC52` — a `Copy`/`Cut` over a
/// non-empty selection must emit an OSC 52 raw chunk whose decoded payload
/// byte-equals that selection's text, computed the same way
/// `commands::clipboard::extract_copy_text` does: the plain half-open
/// `selection_range()`.
///
/// Active-document-switch-safe: reads only `prev` plus `ctx.raw` from the
/// SAME step — there is no `next` to compare against, so a document switch
/// has nothing to disagree about here.
pub fn clip_osc52(prev: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    let MsgTag::Key {
        command: Some(cmd), ..
    } = &ctx.msg
    else {
        return None;
    };
    if !matches!(cmd, Command::Copy | Command::Cut) {
        return None;
    }
    // `ctx.msg.command` is `keymap::resolve(input)` computed unconditionally
    // against the editor's own binding table — resolving a key never
    // depends on which pane is focused, only on how it's later routed. With
    // focus on the Explorer or the Open Tabs pane, that pane consumes ⌘C
    // itself and no copy happens at all — a component ignores keys when
    // unfocused. The title is different: it resolves through this
    // SAME table (decision 3) and DOES copy on ⌘C, but its own name, taken
    // from the field's window (assumption A2), not from a document cursor's
    // `selection_range()` — a convention this checker does not model.
    // Either way, without this guard the checker would assert a
    // payload the focused pane never produced. This is the same scoping
    // `pane_no_bleed` applies. (Latent since Explorer/Tabs focus existed;
    // the Up-at-editor-top-focuses-the-title gesture made it easy to
    // reach.)
    if prev.modal_open || prev.focus != Pane::Editor || prev.focus_target != FocusTarget::Editor {
        return None;
    }
    let [cursor] = prev.cursors.as_slice() else {
        return None;
    };
    if !cursor.has_selection() {
        return None;
    }

    let buf = Buffer::new(prev.content.clone());
    let (start, end) = cursor.selection_range();
    let expected = buf.slice(start.get(), end.get())?;
    if expected.is_empty() {
        return None;
    }

    let found = ctx
        .raw
        .iter()
        .any(|bytes| decode_osc52(bytes).as_deref() == Some(expected.as_bytes()));
    if found {
        return None;
    }
    Some(Violation::new(
        "CLIP-OSC52",
        format!(
            "no OSC 52 raw chunk decoded to the selected text {:?}; raw chunks emitted: {}",
            trunc(expected, 80),
            ctx.raw.len()
        ),
    ))
}
