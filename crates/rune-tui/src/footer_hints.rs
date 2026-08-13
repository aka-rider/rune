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

/// Default-mode hints, CONTEXTUAL per focused pane (plan WP6.S2,
/// superseding WP2.S6/S7's blind `GLOBAL_BINDINGS` iteration): a
/// priority-ordered `(label, help, active)` list —
///
/// 1. `⌘S save` — only in the Editor, and only when the active document
///    isn't `ReadOnly::Always` (plan WP6: that variant can never be
///    saved); styled active only when the document is dirty (assumption
///    A2: for every document where it's PRESENT it's never removed, so
///    the row never jumps there).
/// 2. Every remaining non-alias `GLOBAL_BINDINGS` entry except `⌘S` (already
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

    // Plan WP6: keyed on the `ReadOnly` VARIANT, not on `is_read_only()`
    // and not on dirtiness. `Document::is_dirty()` returns the render-only
    // `is_dirty_cached` field, while `save::trigger_save` deliberately
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
    // the toggle and keeps a live, working `⌘S`, so it keeps the hint.
    if app.focus() == Pane::Editor
        && !matches!(
            app.active_doc().read_only,
            ReadOnly::Always | ReadOnly::Preview
        )
        && let Some((label, help)) = crate::global::hint_for(GlobalCommand::Save)
    {
        entries.push((label, help, app.is_dirty()));
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
        Pane::Editor => {}
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
    use rune_vfs::{Mem, Vfs};
    use std::sync::Arc;

    fn app_with(content: &str) -> App {
        App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
    }

    /// WP5.S7 — while the title is focused with the extension gate locked
    /// and an extension actually present, the footer offers both the
    /// unlock gesture and the commit itself.
    #[test]
    fn title_focus_hints_show_the_unlock_gesture_while_locked_with_an_extension() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(std::path::Path::new("/root/a.md"), b"hi")
            .expect("seed a.md");
        let vfs: Arc<dyn rune_vfs::Vfs + Send + Sync> = mem;
        let mut app = App::new(
            Buffer::new("hi"),
            Some(std::path::PathBuf::from("/root/a.md")),
            vfs,
            None,
        );
        app.focus_title();
        assert!(!app.title.ext_unlocked(), "seeded with a stem: locked");

        let entries = default_hint_entries(&app);
        assert!(
            entries
                .iter()
                .any(|(label, help, _)| label == "\u{2192}" && *help == "extension"),
            "expected an unlock hint, got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(label, help, _)| label == "\u{23ce}" && *help == "rename"),
            "expected a rename hint, got {entries:?}"
        );
    }

    /// Once the gate is unlocked (or there's no extension to unlock — a
    /// pathless draft's own seeded state, decision 9), only the commit hint
    /// remains: offering an unlock gesture that's already a no-op would be
    /// misleading.
    #[test]
    fn title_focus_hints_drop_the_unlock_gesture_once_unlocked() {
        let mut app = app_with("hi");
        app.focus_title();
        assert!(app.title.ext_unlocked(), "a draft seeds unlocked");

        let entries = default_hint_entries(&app);
        assert!(
            !entries.iter().any(|(_, help, _)| *help == "extension"),
            "expected no unlock hint once unlocked, got {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|(label, help, _)| label == "\u{23ce}" && *help == "rename"),
            "expected a rename hint, got {entries:?}"
        );
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

    /// `GlobalCommand::Merge` has exactly one live binding (`^M` — Ghostty
    /// steals `⌘M`, so that form was dropped rather than bound), so on a
    /// diverged document the default hints list "merge" exactly once,
    /// rendered with the `^` glyph, never `⌘`.
    #[test]
    fn default_hints_list_merge_once_as_ctrl_m() {
        let mut app = app_with("hello");
        app.active_doc_mut().last_sync = Some(rune_db::SyncKind::Diverged);
        let entries = default_hint_entries(&app);
        let mut merge_entries = entries.iter().filter(|(_, help, _)| *help == "merge");
        let (label, _, _) = merge_entries.next().expect("expected a merge hint entry");
        assert_eq!(label, "^M");
        assert!(
            merge_entries.next().is_none(),
            "expected exactly one merge hint, got {entries:?}"
        );
    }

    /// Without disk-side divergence there is nothing `^M` can merge —
    /// `merge::begin` refuses — so the hint stays out of the default row
    /// for every non-diverged sync state.
    #[test]
    fn default_hints_omit_merge_without_divergence() {
        for last_sync in [
            None,
            Some(rune_db::SyncKind::Clean),
            Some(rune_db::SyncKind::BufferAhead),
        ] {
            let mut app = app_with("hello");
            app.active_doc_mut().last_sync = last_sync;
            let entries = default_hint_entries(&app);
            assert!(
                !entries.iter().any(|(_, help, _)| *help == "merge"),
                "expected no merge hint for {last_sync:?}, got {entries:?}"
            );
        }
    }

    /// Plan WP6 — the chord is dead for `ReadOnly::Always` (an image
    /// document is refused on `kind`, the Help tab has no `file_path`, the
    /// error banner is never in `app.documents`), so the hint must not
    /// promise it.
    #[test]
    fn default_hint_entries_omit_save_for_a_read_only_always_document() {
        let save_label = GLOBAL_BINDINGS
            .iter()
            .find(|b| matches!(b.cmd, GlobalCommand::Save))
            .expect("a Save binding exists")
            .label();

        let mut app = app_with("hello");
        app.active_doc_mut().read_only = crate::document::ReadOnly::Always;

        let entries = default_hint_entries(&app);
        assert!(
            !entries.iter().any(|(label, _, _)| *label == save_label),
            "expected no save hint for ReadOnly::Always, got {entries:?}"
        );
    }

    /// Plan WP6 — a `ReadOnly::Reading` document may hold bytes typed
    /// before the toggle and keeps a live `⌘S`, so the hint stays.
    #[test]
    fn default_hint_entries_keep_save_for_a_read_only_reading_document() {
        let save_label = GLOBAL_BINDINGS
            .iter()
            .find(|b| matches!(b.cmd, GlobalCommand::Save))
            .expect("a Save binding exists")
            .label();

        let mut app = app_with("hello");
        app.active_doc_mut().read_only = crate::document::ReadOnly::Reading;

        let entries = default_hint_entries(&app);
        assert!(
            entries.iter().any(|(label, _, _)| *label == save_label),
            "expected a save hint for ReadOnly::Reading, got {entries:?}"
        );
    }

    /// A `Preview` document promises none of save, close, or rename in the
    /// footer: all three refuse it (`save::trigger_save`, `workspace::
    /// request_close`, `App::focus_title`), so none may appear as a live
    /// hint.
    #[test]
    fn default_hint_entries_omit_save_close_and_rename_for_a_preview_document() {
        let save_label = GLOBAL_BINDINGS
            .iter()
            .find(|b| matches!(b.cmd, GlobalCommand::Save))
            .expect("a Save binding exists")
            .label();
        let close_label = GLOBAL_BINDINGS
            .iter()
            .find(|b| matches!(b.cmd, GlobalCommand::CloseFile))
            .expect("a CloseFile binding exists")
            .label();
        let rename_label = GLOBAL_BINDINGS
            .iter()
            .find(|b| matches!(b.cmd, GlobalCommand::FocusTitle))
            .expect("a FocusTitle binding exists")
            .label();

        let mut app = app_with("hello");
        app.active_doc_mut().read_only = crate::document::ReadOnly::Preview;

        let entries = default_hint_entries(&app);
        assert!(
            !entries.iter().any(|(label, _, _)| *label == save_label),
            "expected no save hint for ReadOnly::Preview, got {entries:?}"
        );
        assert!(
            !entries.iter().any(|(label, _, _)| *label == close_label),
            "expected no close hint for ReadOnly::Preview, got {entries:?}"
        );
        assert!(
            !entries.iter().any(|(label, _, _)| *label == rename_label),
            "expected no rename hint for ReadOnly::Preview, got {entries:?}"
        );
    }

    /// The mirror of the above: an ordinary `ReadOnly::No` document keeps
    /// all three hints, so the suppression above is really keyed on
    /// `Preview` and not accidentally dropping them for everyone.
    #[test]
    fn default_hint_entries_keep_save_close_and_rename_for_an_ordinary_document() {
        let save_label = GLOBAL_BINDINGS
            .iter()
            .find(|b| matches!(b.cmd, GlobalCommand::Save))
            .expect("a Save binding exists")
            .label();
        let close_label = GLOBAL_BINDINGS
            .iter()
            .find(|b| matches!(b.cmd, GlobalCommand::CloseFile))
            .expect("a CloseFile binding exists")
            .label();
        let rename_label = GLOBAL_BINDINGS
            .iter()
            .find(|b| matches!(b.cmd, GlobalCommand::FocusTitle))
            .expect("a FocusTitle binding exists")
            .label();

        let app = app_with("hello");
        assert_eq!(app.active_doc().read_only, crate::document::ReadOnly::No);

        let entries = default_hint_entries(&app);
        assert!(
            entries.iter().any(|(label, _, _)| *label == save_label),
            "expected a save hint for ReadOnly::No, got {entries:?}"
        );
        assert!(
            entries.iter().any(|(label, _, _)| *label == close_label),
            "expected a close hint for ReadOnly::No, got {entries:?}"
        );
        assert!(
            entries.iter().any(|(label, _, _)| *label == rename_label),
            "expected a rename hint for ReadOnly::No, got {entries:?}"
        );
    }

    /// While the fuzzy file finder is open, the default hints reflect
    /// `FILESEARCH_BINDINGS`, not the ordinary Explorer table — even though
    /// `app.focus()` itself still reads `Pane::Explorer` throughout.
    #[test]
    fn filesearch_open_shows_its_own_hints_not_the_explorer_s() {
        let mut app = app_with("hello");
        app.frame_width = 120;
        app.frame_height = 34;
        let mut effects = crate::runtime::Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        assert_eq!(app.focus(), Pane::Explorer, "test setup");

        let entries = default_hint_entries(&app);
        assert!(
            entries.iter().any(|(_, help, _)| *help == "type to filter"),
            "expected the finder's own hints, got {entries:?}"
        );
        assert!(
            !entries.iter().any(|(_, help, _)| *help == "up dir"),
            "the Explorer's own hint must not leak in while the finder is open: {entries:?}"
        );
    }
}
