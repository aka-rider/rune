//! The default-mode hint list/span builders, split out of [`crate::footer`]
//! to keep it under the 500-line budget: [`default_hint_entries`] (the
//! priority-ordered `(label, help, active)` list, contextual per focused
//! pane), [`hint_entry_spans`] (one entry's own span group), and the two
//! renderers built from them — [`default_hint_spans`] (full, untruncated)
//! and [`truncated_default_hint_spans`] (width-truncated, for `draw`).

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
use crate::width::display_width;

/// `binding`'s label, built into the shared `buf` (cleared first) and cloned
/// out — reusing `buf` across the whole hint list lets its capacity settle
/// at the longest label seen instead of every call growing its own `String`
/// from empty.
fn labeled<C: Copy + 'static>(binding: &Binding<C>, buf: &mut String) -> String {
    buf.clear();
    binding.write_label(buf);
    buf.clone()
}

/// Default-mode hints, CONTEXTUAL per focused pane (supersedes a blind
/// `GLOBAL_BINDINGS` iteration): a priority-ordered `(label, help, active)`
/// list —
///
/// 1. `^S save` — only in the Editor, and only when the active document
///    isn't `ReadOnly::Always` (that variant can never be
///    saved); styled active only when the document is dirty (assumption
///    A2: for every document where it's PRESENT it's never removed, so
///    the row never jumps there).
/// 2. Every remaining non-alias `GLOBAL_BINDINGS` entry except `^S` (already
///    placed at step 1, with its own dirty-state styling) — help and quit
///    are always-available actions, so they get a stable position ahead of
///    the pane-specific table below rather than being the first thing width
///    truncation drops. Aliased chords (the ⌘/`^` form the footer doesn't
///    need twice, or a second quit chord) are skipped: they still work,
///    `help_markdown` still lists them, but the footer only needs to name a
///    command once.
/// 3. The focused pane's own table (`EXPLORER_BINDINGS`/`TABS_BINDINGS`) —
///    nothing extra for the Editor, whose chords live in `keymap::resolve`,
///    a match rather than a table (the same asymmetry `help.rs` already
///    records). Placed last: a pane's own chords are the ones that should
///    drop first under width pressure, not the always-available global tail.
///
/// One source read by both the untruncated renderer (`footer_text`,
/// `rune-fuzz`'s snapshots) and the width-truncated one `draw` uses, so the
/// two can never disagree about WHAT the hints are, only how many fit.
pub(crate) fn default_hint_entries(app: &App) -> Vec<(String, &'static str, bool)> {
    let mut entries: Vec<(String, &'static str, bool)> = Vec::new();
    let mut label_buf = String::new();

    // Keyed on the `ReadOnly` VARIANT, not on `is_read_only()`
    // and not on dirtiness. `Document::dirty_for_render()` returns the
    // render-only `is_dirty_cached` field, while `save::trigger_save` deliberately
    // re-derives via `materialize_ack::is_dirty_now` instead of reading
    // that cache — a dirtiness-keyed hint would promise a chord the save
    // path doesn't actually honour whenever the cache is stale, and
    // `render` cannot take `&mut App` to refresh it first. `ReadOnly::
    // Always` documents can never be saved regardless: an image document
    // is refused on `kind == DocumentKind::Image` in `save.rs`, the Help
    // tab has `file_path: None` and only reaches the `NeedsName` arm, and
    // the error-banner document is never inserted into `app.documents` at
    // all — so dropping the hint there drops it exactly where the chord
    // is dead. A `ReadOnly::Reading` document may hold bytes typed before
    // the toggle and keeps a live, working `^S`, so it keeps the hint.
    if focus::target(app) == FocusTarget::Editor
        && !matches!(
            app.active_doc().read_only,
            ReadOnly::Always | ReadOnly::Preview
        )
        && let Some((label, help)) = crate::global::hint_for(GlobalCommand::Save)
    {
        entries.push((label, help, app.dirty_for_render()));
    }

    entries.extend(
        GLOBAL_BINDINGS
            .iter()
            .filter(|b| !b.alias && !matches!(b.cmd, GlobalCommand::Save))
            .filter(|b| !hint_suppressed(app, b.cmd))
            .map(|b| (labeled(b, &mut label_buf), b.help, true)),
    );

    // The finder is never a `Pane` (chrome stays `Explorer` throughout), so
    // this has to be checked ahead of the `app.focus()` match below, or its
    // rows would always read as ordinary Explorer hints. Reflection over
    // `FILESEARCH_BINDINGS` keeps this from drifting out of step with the
    // table `filesearch::keys::handle_key` actually resolves against;
    // aliased rows are skipped, same as `GLOBAL_BINDINGS` above.
    if focus::target(app) == FocusTarget::FileSearch {
        entries.extend(
            FILESEARCH_BINDINGS
                .iter()
                .filter(|b| !b.alias)
                .map(|b| (labeled(b, &mut label_buf), b.help, true)),
        );
        return entries;
    }

    match app.focus() {
        Pane::Explorer => entries.extend(
            EXPLORER_BINDINGS
                .iter()
                .map(|b| (labeled(b, &mut label_buf), b.help, true)),
        ),
        Pane::Tabs => entries.extend(
            TABS_BINDINGS
                .iter()
                .map(|b| (labeled(b, &mut label_buf), b.help, true)),
        ),
        // The title field has no binding TABLE of its own — `title::keys::
        // handle_key` matches Enter/Esc/editing keys directly (they are a
        // text field's own behaviour, not chords worth enumerating in the
        // Help doc) — but it does have two gestures worth surfacing here:
        // the Right-at-end-of-stem unlock (only while there's an extension
        // left to unlock) and the commit itself. The global hints above
        // still render while renaming.
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
                        .filter(|b| !b.alias)
                        .map(|b| (labeled(b, &mut label_buf), b.help, true)),
                );
            }
        }
        // The messages pane's own keys render through `footer::Mode::
        // Messages` instead — `mode()` returns that variant before
        // `DefaultHints` is ever reached while this pane holds
        // focus, so this arm is unreachable in practice; it exists only to
        // keep this match exhaustive.
        Pane::Messages => {}
    }

    entries
}

/// The one place that suppresses a `GLOBAL_BINDINGS` hint the bulk
/// `extend` above would otherwise show unconditionally: a `Preview`
/// document refuses close (`workspace::request_close`) and rename entry
/// (`App::focus_title`) alike, so their hints must not promise a chord that
/// only sets a status message; and `merge` is offered only while the active
/// document's last known sync classification actually has disk-side changes
/// to merge (`DiskAhead`/`Diverged`) — `merge::begin` refuses every other
/// state, so the hint would promise a chord that only posts a refusal.
/// `Save` already has its own bespoke arm above (styled by dirtiness, not
/// just present/absent) and is filtered out of this bulk `extend` before it
/// ever reaches here.
fn hint_suppressed(app: &App, cmd: GlobalCommand) -> bool {
    if app.active_doc().read_only == ReadOnly::Preview
        && matches!(cmd, GlobalCommand::CloseFile | GlobalCommand::FocusTitle)
    {
        return true;
    }
    matches!(cmd, GlobalCommand::Merge)
        && !app
            .active_doc()
            .last_sync
            .is_some_and(rune_db::SyncKind::is_disk_divergent)
}

/// One hint entry's own span group: a leading `"  "` separator (every
/// entry but the first), the key label (active/inactive style), a space,
/// then the help text. The chokepoint both `default_hint_spans` (full,
/// untruncated) and `draw`'s width-truncated renderer build from, so an
/// entry can never render differently in the two paths.
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

/// The FULL, untruncated hint spans — what `footer_text`
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

/// Priority-truncated hint spans: reserves room for
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
#[path = "footer_hints_tests.rs"]
mod tests;
