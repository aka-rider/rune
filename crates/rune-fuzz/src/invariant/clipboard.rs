//! `PASTE-VERBATIM`/`CLIP-OSC52` — the two clipboard-path byte-verbatim
//! invariants.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use rune_core::buffer::Buffer;
use rune_tui::commands::nav;
use rune_tui::keymap::Command;
use rune_tui::pane::Pane;
use rune_tui::runtime::PasteTarget;

use super::{Violation, trunc};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// `PASTE-VERBATIM` — on a non-empty `Paste`/`ClipboardRead` into
/// a single cursor, `next.content` must equal `prev.content` with exactly
/// the pasted text's bytes substituted at the cursor: inserted at the
/// caret when collapsed, or replacing `[selection_start, selection_end_
/// inclusive)` when there's a selection (CODE-REVIEW.md rune-fuzz finding
/// 12) — `commands::edit::insert_text`'s `per_cursor_selection_edits`
/// (via `per_cursor_selection_edits`'s own `has_selection` arm) uses the
/// SAME `nav::selection_end_inclusive` reversed-selection nudge
/// `clip_osc52` below already accounts for; it is a shared selection-
/// range convention for every selection-consuming command in this crate,
/// not a copy-extraction-only rule. `handle_paste_content` inserts
/// unfiltered (`commands/clipboard.rs`), the ONE path that can carry
/// control bytes at all (G3). Inert on a `read_only` document (the Help
/// virtual document, reachable since `F1` joined `arb_any_keycode` —
/// CODE-REVIEW.md rune-fuzz finding 9): every mutating command chokepoint
/// refuses a read-only document by construction, so a paste there
/// correctly inserts nothing, and asserting verbatim insertion anyway
/// would be asserting a property production never claimed to begin with.
///
/// Title-targeted-safe: the title field has its own in-memory buffer, out
/// of this checker's domain entirely (it is unjournaled and never
/// reaches the document). A `MsgTag::Paste` step only asserts when
/// `prev.focus == Pane::Editor` (a title-focused `Paste` never touches
/// `app.active`), and a `MsgTag::ClipboardRead` step only asserts when its
/// captured `target` names `prev.active` (a `PasteTarget::Title` reply
/// inserts into the field, not the document). Both guards are also what
/// keeps `prev.active == next.active`, since neither guarded path can
/// switch the active document.
pub fn paste_verbatim(prev: &Snapshot, next: &Snapshot, ctx: &StepCtx) -> Option<Violation> {
    let text = match &ctx.msg {
        MsgTag::Paste(t) if prev.focus == Pane::Editor => t,
        MsgTag::ClipboardRead { text, target } if *target == PasteTarget::Document(prev.active) => {
            text
        }
        _ => return None,
    };
    if text.is_empty() || prev.read_only != rune_tui::document::ReadOnly::No {
        return None;
    }
    let [cursor] = prev.cursors.as_slice() else {
        return None;
    };
    let buf = Buffer::new(prev.content.clone());
    let start = cursor.selection_start();
    let end = nav::selection_end_inclusive(cursor, &buf);
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
/// `commands::clipboard::extract_copy_text` does (`selection_start()` ..
/// `nav::selection_end_inclusive`, which nudges a REVERSED selection's end
/// past its anchor unless that byte is a newline).
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
    // `nav::selection_end_inclusive` range — a convention this checker does
    // not model. Either way, without this guard the checker would assert a
    // payload the focused pane never produced. This is the same scoping
    // `pane_no_bleed` applies. (Latent since Explorer/Tabs focus existed;
    // the Up-at-editor-top-focuses-the-title gesture made it easy to
    // reach.)
    if prev.modal_open || prev.focus != Pane::Editor {
        return None;
    }
    let [cursor] = prev.cursors.as_slice() else {
        return None;
    };
    if !cursor.has_selection() {
        return None;
    }

    let buf = Buffer::new(prev.content.clone());
    let start = cursor.selection_start();
    let end = nav::selection_end_inclusive(cursor, &buf);
    let expected = buf.slice(start, end)?;
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
