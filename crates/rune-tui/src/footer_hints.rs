use ratatui::text::Span;

use crate::app::App;
use crate::binding::Binding;
use crate::document::ReadOnly;
use crate::explorer_keys::EXPLORER_BINDINGS;
use crate::filesearch::keys::FILESEARCH_BINDINGS;
use crate::focus::{self, FocusTarget};
use crate::keymap::{GLOBAL_BINDINGS, GlobalCommand};
use crate::opentabs::TABS_BINDINGS;
use crate::pane::Pane;
use crate::registry::{self, Availability, CommandId};
use crate::width::display_width;

// Reuses the shared `buf` across the whole hint list so its capacity
// settles at the longest label seen, instead of every call growing its
// own `String` from empty.
fn labeled<C: Copy + 'static>(binding: &Binding<C>, buf: &mut String) -> String {
    buf.clear();
    binding.write_label(buf);
    buf.clone()
}

/// Default-mode hints, contextual per focused pane rather than a blind
/// `GLOBAL_BINDINGS` walk: a priority-ordered `(label, help, active)` list,
/// pane-specific chords placed last so they are the first thing width
/// truncation drops, not the always-available global tail. Read by both
/// the untruncated renderer and the width-truncated one `draw` uses, so
/// the two can never disagree about WHAT the hints are, only how many fit.
pub(crate) fn default_hint_entries(app: &App) -> Vec<(String, &'static str, bool)> {
    let mut entries: Vec<(String, &'static str, bool)> = Vec::new();
    let mut label_buf = String::new();

    // Keyed on the `ReadOnly` variant, never on dirtiness: the label
    // itself must stay reachable whenever the chord is live, independent
    // of whatever the document's bytes are doing right now.
    if focus::target(app) == FocusTarget::Editor
        && !matches!(
            app.active_doc().read_only,
            ReadOnly::Always | ReadOnly::Preview
        )
        && let Some((label, _)) = crate::global::hint_for(GlobalCommand::Save)
        && let Some(spec) = registry::spec(CommandId::Global(GlobalCommand::Save))
    {
        entries.push((label, spec.help, app.is_dirty()));
    }

    entries.extend(
        GLOBAL_BINDINGS
            .iter()
            .filter(|b| !b.secondary && !matches!(b.cmd, GlobalCommand::Save))
            .filter(|b| {
                registry::availability(app, CommandId::Global(b.cmd)) == Availability::Available
            })
            .filter_map(|b| {
                let spec = registry::spec(registry::rows::global::adapt(b.cmd))?;
                Some((labeled(b, &mut label_buf), spec.help, true))
            }),
    );

    // The finder is never a `Pane` (chrome stays `Explorer` throughout), so
    // this has to be checked ahead of the `app.focus()` match below, or its
    // rows would always read as ordinary Explorer hints.
    if focus::target(app) == FocusTarget::FileSearch {
        entries.extend(
            FILESEARCH_BINDINGS
                .iter()
                .filter(|b| !b.secondary)
                .filter_map(|b| {
                    let spec = registry::spec(registry::rows::pane::adapt_filesearch(b.cmd))?;
                    Some((labeled(b, &mut label_buf), spec.help, true))
                }),
        );
        return entries;
    }

    if focus::target(app) == FocusTarget::Palette {
        entries.push(("\u{238b}".to_string(), "close", true));
        entries.push(("\u{23ce}".to_string(), "run", true));
        entries.push(("\u{2191}\u{2193}".to_string(), "navigate", true));
        return entries;
    }

    match app.focus() {
        Pane::Explorer => entries.extend(
            EXPLORER_BINDINGS
                .iter()
                .filter(|b| !b.secondary)
                .filter_map(|b| {
                    let spec = registry::spec(registry::rows::pane::adapt_explorer(b.cmd))?;
                    Some((labeled(b, &mut label_buf), spec.help, true))
                }),
        ),
        Pane::Tabs => entries.extend(TABS_BINDINGS.iter().filter_map(|b| {
            let spec = registry::spec(registry::rows::pane::adapt_tabs(b.cmd))?;
            Some((labeled(b, &mut label_buf), spec.help, true))
        })),
        // The title field has no binding table of its own — its keys are
        // matched directly in `title::keys::handle_key` — but the
        // Right-at-end-of-stem unlock and the commit are worth surfacing.
        Pane::Title => {
            if crate::title::keys::can_unlock_extension(&app.title) {
                entries.push(("\u{2192}".to_string(), "extension", true));
            }
            entries.push(("\u{23ce}".to_string(), "rename", true));
        }
        Pane::Editor => {
            if app
                .diff
                .as_ref()
                .is_some_and(|diff| diff.right == app.active)
            {
                entries.extend(
                    crate::diff_view::keys::DIFF_BINDINGS
                        .iter()
                        .filter(|b| !b.secondary)
                        .filter_map(|b| {
                            let spec = registry::spec(registry::rows::pane::adapt_diff(b.cmd))?;
                            Some((labeled(b, &mut label_buf), spec.help, true))
                        }),
                );
            }
        }
        // Unreachable in practice — `footer::mode()` returns `Mode::
        // Messages` before `DefaultHints` while this pane holds focus —
        // kept only so this match stays exhaustive.
        Pane::Messages => {}
    }

    entries
}

/// The chokepoint both the full and width-truncated renderers build one
/// entry from, so an entry can never render differently in the two paths.
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

/// The full, untruncated hint spans. Truncation happens only inside
/// `draw`, so these stay width-independent.
pub(crate) fn default_hint_spans(app: &App) -> Vec<Span<'static>> {
    default_hint_entries(app)
        .into_iter()
        .enumerate()
        .flat_map(|(i, (label, help, active))| hint_entry_spans(&app.theme, i, label, help, active))
        .collect()
}

/// Reserves room for the position readout (`right_width`) first, then
/// appends whole entries in priority order only while the next one still
/// fits — never a partial entry.
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
#[path = "footer_hints_tests.rs"]
mod tests;
