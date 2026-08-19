#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::app::App;
use crate::keymap::GlobalCommand;
use rune_core::buffer::Buffer;
use rune_vfs::{Mem, VfsTestExt};
use std::sync::Arc;

fn app_with(content: &str) -> App {
    App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
}

/// While the title is focused with the extension gate locked
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

/// The span-level regression guard for assumption A2: the
/// `^S` label span carries `theme.chrome.footer_key_inactive` on a
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
    assert!(!app.dirty_for_render());
    let spans = default_hint_spans(&app);
    let save_span = spans
        .iter()
        .find(|s| s.content.as_ref() == save_label)
        .expect("save label span present");
    assert_eq!(save_span.style, app.theme.chrome.footer_key_inactive);

    let mut app = app_with("hello");
    app.active_doc_mut().is_dirty_cached = true;
    assert!(app.dirty_for_render());
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

/// The chord is dead for `ReadOnly::Always` (an image
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

/// A `ReadOnly::Reading` document may hold bytes typed
/// before the toggle and keeps a live `^S`, so the hint stays.
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

#[test]
fn diff_view_hints_show_only_while_a_diff_view_is_active_on_the_focused_document() {
    let mut app = app_with("right text");
    let entries = default_hint_entries(&app);
    assert!(
        !entries.iter().any(|(_, help, _)| *help == "next hunk"),
        "no diff hint before a diff view exists: {entries:?}"
    );

    crate::diff_view::install(&mut app, b"left text".to_vec(), "fileA.md".to_string())
        .expect("fixture is valid UTF-8");
    let entries = default_hint_entries(&app);
    assert!(
        entries.iter().any(|(_, help, _)| *help == "next hunk"),
        "expected a diff hint once the diff view is active: {entries:?}"
    );
}
