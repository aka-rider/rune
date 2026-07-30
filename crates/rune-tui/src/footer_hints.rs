//! The default-mode hint list/span builders, split out of [`crate::footer`]
//! to keep it under the §1.6 line budget: [`default_hint_entries`] (the
//! priority-ordered `(label, help, active)` list, contextual per focused
//! pane), [`hint_entry_spans`] (one entry's own span group), and the two
//! renderers built from them — [`default_hint_spans`] (full, untruncated)
//! and [`truncated_default_hint_spans`] (width-truncated, for `draw`).

use ratatui::text::Span;

use crate::app::App;
use crate::explorer_keys::EXPLORER_BINDINGS;
use crate::global::{self, LEADER_BINDINGS};
use crate::keymap::{GLOBAL_BINDINGS, GlobalCommand};
use crate::opentabs::TABS_BINDINGS;
use crate::pane::Pane;
use crate::width::display_width;

/// Default-mode hints, now CONTEXTUAL per focused pane (plan WP6.S2,
/// superseding WP2.S6/S7's blind `GLOBAL_BINDINGS` iteration): a
/// priority-ordered `(label, help, active)` list —
///
/// 1. The three held-space leader chords (`global::LEADER_BINDINGS`),
///    always active.
/// 2. `⌘S save` — only in the Editor; styled active only when the
///    document is dirty (assumption A2: always PRESENT there, never
///    removed, so the row never jumps).
/// 3. Every remaining non-alias `GLOBAL_BINDINGS` entry except `⌘S` (already
///    placed at step 2, with its own dirty-state styling) — help and quit
///    are always-available actions, so they get a stable position ahead of
///    the pane-specific table below rather than being the first thing width
///    truncation drops. Aliased ctrl fallbacks (a chord that duplicates a
///    leader chord, or a second quit chord) are skipped: they still work,
///    `help_markdown` still lists them, but the footer only needs to name a
///    command once.
/// 4. The focused pane's own table (`EXPLORER_BINDINGS`/`TABS_BINDINGS`) —
///    nothing extra for the Editor, whose chords live in `keymap::resolve`,
///    a match rather than a table (the same asymmetry `help.rs` already
///    records). Placed last: a pane's own chords are the ones that should
///    drop first under width pressure, not the always-available global tail.
///
/// One source read by both the untruncated renderer (`footer_text`,
/// `rune-fuzz`'s snapshots) and the width-truncated one `draw` uses, so the
/// two can never disagree about WHAT the hints are, only how many fit.
pub(crate) fn default_hint_entries(app: &App) -> Vec<(String, &'static str, bool)> {
    let mut entries: Vec<(String, &'static str, bool)> = LEADER_BINDINGS
        .iter()
        .map(|b| (global::leader_label(b), b.help, true))
        .collect();

    if app.focus() == Pane::Editor
        && let Some(save) = GLOBAL_BINDINGS
            .iter()
            .find(|b| matches!(b.cmd, GlobalCommand::Save))
    {
        entries.push((save.label(), save.help, app.is_dirty()));
    }

    entries.extend(
        GLOBAL_BINDINGS
            .iter()
            .filter(|b| !b.alias && !matches!(b.cmd, GlobalCommand::Save))
            .map(|b| (b.label(), b.help, true)),
    );

    match app.focus() {
        Pane::Explorer => {
            entries.extend(EXPLORER_BINDINGS.iter().map(|b| (b.label(), b.help, true)))
        }
        Pane::Tabs => entries.extend(TABS_BINDINGS.iter().map(|b| (b.label(), b.help, true))),
        // The title field has no binding TABLE of its own — `title::
        // handle_key` matches Enter/Esc/editing keys directly (they are a
        // text field's own behaviour, not chords worth enumerating in the
        // Help doc), so there is nothing here to reflect over. The global
        // hints above still render while renaming.
        Pane::Title => {}
        Pane::Editor => {}
    }

    entries
}

/// One hint entry's own span group: a leading `"  "` separator (every
/// entry but the first), the key label (active/inactive style), a space,
/// then the help text. The chokepoint both `default_hint_spans` (full,
/// untruncated) and `draw`'s width-truncated renderer build from, so an
/// entry can never render differently in the two paths (plan WP6.S3).
pub(crate) fn hint_entry_spans(
    theme: &crate::theme::Theme,
    index: usize,
    label: String,
    help: &'static str,
    active: bool,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if index > 0 {
        spans.push(Span::styled("  ", theme.chrome.footer_hint));
    }
    let key_style = if active {
        theme.chrome.footer_key
    } else {
        theme.chrome.footer_key_inactive
    };
    spans.push(Span::styled(label, key_style));
    spans.push(Span::styled(" ", theme.chrome.footer_hint));
    spans.push(Span::styled(help, theme.chrome.footer_hint));
    spans
}

/// The FULL, untruncated hint spans (plan WP6.S4) — what `footer_text`
/// asserts on and what `rune-fuzz/src/snapshot.rs` captures into every fuzz
/// snapshot. Truncation happens only inside `draw`, so these stay
/// width-independent and the snapshots stay stable.
pub(crate) fn default_hint_spans(app: &App) -> Vec<Span<'static>> {
    default_hint_entries(app)
        .into_iter()
        .enumerate()
        .flat_map(|(i, (label, help, active))| hint_entry_spans(&app.theme, i, label, help, active))
        .collect()
}

/// Priority-truncated hint spans (plan WP6.S3, risk R3): reserves room for
/// `Ln n, Col n` (`right_width`) FIRST, then appends whole entries in
/// priority order only while the next one still fits the remaining width —
/// never a partial entry, so the position readout can never fall off the
/// row.
pub(crate) fn truncated_default_hint_spans(
    app: &App,
    available: usize,
    right_width: usize,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for (i, (label, help, active)) in default_hint_entries(app).into_iter().enumerate() {
        let entry = hint_entry_spans(&app.theme, i, label, help, active);
        let entry_width: usize = entry.iter().map(|s| display_width(&s.content)).sum();
        if used + entry_width + right_width > available {
            break;
        }
        used += entry_width;
        spans.extend(entry);
    }
    spans
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::keymap::GlobalCommand;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_with(content: &str) -> App {
        App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
    }

    /// Plan WP6.S6 — the span-level regression guard for assumption A2: the
    /// `⌘S` label span carries `theme.chrome.footer_key_inactive` on a
    /// clean document and `theme.chrome.footer_key` once an edit makes it
    /// dirty.
    #[test]
    fn save_label_span_is_styled_inactive_when_clean_and_active_when_dirty() {
        let save_label = GLOBAL_BINDINGS
            .iter()
            .find(|b| matches!(b.cmd, GlobalCommand::Save))
            .expect("a Save binding exists")
            .label();

        let app = app_with("hello");
        assert!(!app.is_dirty());
        let spans = default_hint_spans(&app);
        let save_span = spans
            .iter()
            .find(|s| s.content.as_ref() == save_label)
            .expect("save label span present");
        assert_eq!(save_span.style, app.theme.chrome.footer_key_inactive);

        let mut app = app_with("hello");
        app.active_doc_mut().is_dirty_cached = true;
        assert!(app.is_dirty());
        let spans = default_hint_spans(&app);
        let save_span = spans
            .iter()
            .find(|s| s.content.as_ref() == save_label)
            .expect("save label span present");
        assert_eq!(save_span.style, app.theme.chrome.footer_key);
    }
}
