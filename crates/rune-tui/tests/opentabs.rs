//! WP5 "Done when" tests: Open Tabs rendering/switching, and the close
//! guard's three resolutions (`[S]ave`, `[D]iscard`, `Esc`), driven against
//! a `Mem` vfs seeded with two files. The GLOBAL `^w`/`^1`-`^0` binding
//! tests live in the sibling `opentabs_global.rs` (TODO.md's 500-line budget);
//! both pull shared fixtures from `opentabs_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod opentabs_common;

use rune_tui::app::{self, App};
use rune_tui::commands::edit;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::testgrid;
use rune_tui::{opentabs, workspace};

use opentabs_common::{HEIGHT, WIDTH, app_with, key, open_second, seeded_vfs};

fn plain(code: KeyCode) -> KeyInput {
    key(code, Mods::NONE)
}

fn frame_text(app: &App) -> String {
    testgrid::grid(app, WIDTH, HEIGHT).concat()
}

/// Opening two documents populates `tabs.order`, and both render with their
/// digit shortcut and name below the `Open` divider row.
#[test]
fn tabs_render_both_open_documents_with_digit_shortcuts() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    open_second(&mut app);
    assert_eq!(app.tabs.order.len(), 2);

    app.splits.left.show();
    app.set_focus_pane(Pane::Tabs, &mut Effects::default());
    app.sync_view();

    let text = frame_text(&app);
    assert!(
        text.contains("1:"),
        "expected the first tab's shortcut '1:' in:\n{text}"
    );
    assert!(
        text.contains("2:"),
        "expected the second tab's shortcut '2:' in:\n{text}"
    );
    assert!(text.contains("a.md"));
    assert!(text.contains("b.md"));
}

/// The Open Tabs section is introduced by a divider ROW inside the left
/// column's single border — there is no separate titled block, so the tab
/// rows follow immediately underneath it.
#[test]
fn the_open_divider_row_precedes_the_tab_rows() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    open_second(&mut app);
    app.splits.left.show();
    app.set_focus_pane(Pane::Tabs, &mut Effects::default());
    app.sync_view();

    let rows = testgrid::grid(&app, WIDTH, HEIGHT);
    let divider = rows
        .iter()
        .position(|r| r.contains(" Open "))
        .unwrap_or_else(|| panic!("expected an Open divider row in:\n{}", rows.join("\n")));

    assert!(
        rows[divider].contains('\u{2500}'),
        "the divider row must be filled out with `\u{2500}`:\n{}",
        rows[divider]
    );
    assert!(
        rows[divider + 1].contains("a.md"),
        "the first tab row must sit directly under the divider:\n{}",
        rows[divider + 1]
    );
    assert!(
        rows[divider + 2].contains("b.md"),
        "the second tab row follows it:\n{}",
        rows[divider + 2]
    );
}

/// Enter on a cursor row switches the active document (plan WP5.S2) —
/// driven through `opentabs::handle_key` directly, the same style
/// `tests/explorer.rs` already uses for its own pane-local assertions.
#[test]
fn enter_switches_the_active_document() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let first = app.active;
    let second = open_second(&mut app);
    app.set_focus_pane(Pane::Editor, &mut Effects::default());
    workspace::switch_to(&mut app, first); // back to a.md, cursor -> index 0

    app.tabs.nav.cursor = 1; // b.md's row
    let outcome = opentabs::handle_key(&mut app, plain(KeyCode::Enter), &mut Effects::default());

    assert_eq!(outcome, rune_tui::keymap::KeyOutcome::Consumed);
    assert_eq!(app.active, second);
    assert_eq!(app.focus(), Pane::Editor);
}

/// A dirty document's tab shows the `x` dirty marker; a clean one shows a
/// blank in its place (plan WP5.S1). The row shape pins the fixed marker
/// columns: pin, dirty, sync (blank here), separator, name.
#[test]
fn dirty_dot_appears_after_an_edit_to_the_active_document() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    app.splits.left.show();
    app.sync_view();
    assert!(
        !frame_text(&app).contains(" x "),
        "test setup: nothing should be dirty yet"
    );

    edit::insert_char(&mut app, second, '!');
    app.sync_view();

    assert!(app.doc(second).unwrap().is_dirty());
    let text = frame_text(&app);
    assert!(
        text.contains(" x  b.md"),
        "expected the dirty marker in b.md's tab row:\n{text}"
    );
}

/// A background tab whose document diverged on disk shows the `⇄` marker
/// in its own fixed column — per-doc state, visible even while a different
/// (clean) document is active, so the footer shows no marker of its own.
#[test]
fn diverged_background_doc_tab_shows_the_sync_marker() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let first = app.active;
    let second = open_second(&mut app);
    workspace::switch_to(&mut app, first);
    app.splits.left.show();
    app.sync_view();
    assert!(
        !frame_text(&app).contains('\u{21c4}'),
        "test setup: nothing should be diverged yet"
    );

    app.doc_mut(second).unwrap().last_sync = Some(rune_db::SyncKind::Diverged);
    app.sync_view();

    let text = frame_text(&app);
    assert!(
        text.contains("\u{21c4} b.md"),
        "expected the sync marker in b.md's tab row:\n{text}"
    );
    assert!(
        !text.contains("\u{21c4} a.md"),
        "the clean a.md row must not carry the marker:\n{text}"
    );
}

/// `request_close` on a dirty document arms the Guard modal, and — like
/// the Error banner before it — every key is consumed at stage 1 while
/// it's up: it never reaches the editor's own buffer.
#[test]
fn request_close_on_a_dirty_doc_arms_the_guard_and_blocks_other_keys() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');
    assert!(app.doc(second).unwrap().is_dirty());

    workspace::request_close(&mut app, second, &mut Effects::default());
    assert!(app.guard.is_some(), "a dirty close must arm a modal");

    let before = app.doc(second).unwrap().buffer.content().to_string();
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Char('q'))), &mut effects);

    assert_eq!(
        app.doc(second).unwrap().buffer.content(),
        before,
        "a key consumed by the Guard must never reach commands::edit"
    );
    assert!(
        app.guard.is_some(),
        "an unbound key must leave the Guard up"
    );
    assert!(app.documents.contains_key(&second), "must not close yet");
}

/// `[D]iscard` closes the document immediately and activates its neighbor.
#[test]
fn discard_closes_and_activates_the_neighbor() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let first = app.active;
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');
    assert_eq!(app.active, second);

    workspace::request_close(&mut app, second, &mut Effects::default());
    assert!(app.guard.is_some());
    // A degraded-save confirm gate armed for `second` and left unresolved
    // (review fix: `close_now` must sweep it, not just `pending_close_on_
    // save`) — must not survive the close as a dangling reference to a
    // document that no longer exists.
    app.pending_save_confirm = Some((second, 0));

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Char('d'))), &mut effects);

    assert!(app.guard.is_none());
    assert!(!app.documents.contains_key(&second), "b.md must be closed");
    assert_eq!(app.documents.len(), 1);
    assert_eq!(app.active, first, "the sole remaining document takes over");
    assert!(!app.tabs.order.contains(&second));
    assert!(
        app.pending_save_confirm.is_none(),
        "a pending_save_confirm targeting the closed doc must be cleared too"
    );
}

/// `[S]ave` triggers a save, closing only once its `Msg::SaveDone` ack
/// reports success — never before, and never on a failure (plan WP5.S3,
/// mind Assumption A1: `db: None` documents take the `SaveDone` fallback
/// path, exercised here since `app_with` builds an `App` with no store).
#[test]
fn save_then_close_waits_for_the_save_done_ack() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');
    assert!(app.doc(second).unwrap().is_dirty());

    workspace::request_close(&mut app, second, &mut Effects::default());
    assert!(app.guard.is_some());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Char('s'))), &mut effects);

    assert!(
        app.guard.is_none(),
        "the Guard clears the moment Save fires"
    );
    assert_eq!(
        app.pending_close_on_save,
        Some(second),
        "a save must actually have started"
    );
    assert!(
        app.documents.contains_key(&second),
        "must not close before the save's ack lands"
    );
    assert_eq!(effects.cmds.len(), 1);
    assert_eq!(effects.cmds[0].kind(), CmdKind::Save);

    let version = app.doc(second).unwrap().buffer.version();
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("the Save Cmd replies with a Msg");
    // Sanity: the driver really did produce `SaveDone` for `second`.
    match &msg {
        Msg::SaveDone { id, .. } => assert_eq!(*id, second),
        other => panic!("expected Msg::SaveDone, got {other:?}"),
    }

    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);

    assert!(
        !app.documents.contains_key(&second),
        "must close once the save's ack reports success"
    );
    assert_eq!(app.pending_close_on_save, None);
    let _ = version;
}

/// A failed `SaveDone` ack must NOT close the document — data safety over
/// honoring a stale close intent.
#[test]
fn a_failed_save_ack_leaves_the_document_open() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');

    workspace::request_close(&mut app, second, &mut Effects::default());
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Char('s'))), &mut effects);
    assert_eq!(app.pending_close_on_save, Some(second));

    let version = app.doc(second).unwrap().buffer.version();
    let mut effects2 = Effects::default();
    app::update(
        &mut app,
        Msg::SaveDone {
            id: second,
            version,
            result: Err("disk full".to_string()),
            durable: true,
        },
        &mut effects2,
    );

    assert!(
        app.documents.contains_key(&second),
        "a failed save must never close the document"
    );
    assert_eq!(app.pending_close_on_save, None);
}

/// `Esc` cancels the Guard, leaving the document and its content untouched.
#[test]
fn escape_cancels_the_guard() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');
    let content_before = app.doc(second).unwrap().buffer.content().to_string();

    workspace::request_close(&mut app, second, &mut Effects::default());
    assert!(app.guard.is_some());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Escape)), &mut effects);

    assert!(app.guard.is_none());
    assert!(app.documents.contains_key(&second));
    assert_eq!(app.doc(second).unwrap().buffer.content(), content_before);
    assert!(
        app.doc(second).unwrap().is_dirty(),
        "still dirty, untouched"
    );
}

/// Escape used to leave the user with no feedback at all — the modal just
/// vanished. Pin that cancelling the dirty-close Guard now names what it
/// cancelled via a status message.
#[test]
fn escape_on_the_dirty_close_guard_sets_a_cancellation_status() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');

    workspace::request_close(&mut app, second, &mut Effects::default());
    assert!(app.guard.is_some());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Escape)), &mut effects);

    assert_eq!(
        rune_tui::messages::newest_text(&app),
        Some("close cancelled")
    );
}

/// Closing the last remaining document (plan WP0) mints a fresh untitled
/// draft rather than refusing — the old refusal made the user open another
/// document just to close the untitled one before they could leave.
#[test]
fn closing_the_only_document_mints_a_fresh_untitled_instead_of_refusing() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let only = app.active;
    assert_eq!(app.documents.len(), 1);

    workspace::request_close(&mut app, only, &mut Effects::default());

    assert!(app.guard.is_none(), "a clean close never arms a Guard");
    assert!(
        !app.documents.contains_key(&only),
        "the original document must actually be gone"
    );
    assert_eq!(
        app.documents.len(),
        1,
        "closing the last document leaves exactly one — a fresh untitled"
    );
    assert_eq!(
        app.active_doc().display_name.as_deref(),
        Some("Untitled 1"),
        "the replacement is the fresh untitled draft, and it's now active"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_none(),
        "there is no more \"can't close\" refusal to report"
    );
}

/// The dirty variant of the same scenario still routes through the close
/// Guard — `^W` on a dirty-and-only document must not silently discard it.
/// `[D]iscard` then lands on the same fresh-untitled replacement.
#[test]
fn closing_a_dirty_only_document_still_routes_through_the_guard() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let only = app.active;
    edit::insert_char(&mut app, only, '!');
    assert!(app.doc(only).unwrap().is_dirty());

    workspace::request_close(&mut app, only, &mut Effects::default());

    assert!(
        app.guard.is_some(),
        "a dirty close still arms the Guard, even for the only document"
    );
    assert!(app.documents.contains_key(&only), "not closed yet");
    assert_eq!(app.documents.len(), 1);

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Char('d'))), &mut effects);

    assert!(app.guard.is_none());
    assert!(!app.documents.contains_key(&only));
    assert_eq!(app.documents.len(), 1);
    assert_eq!(app.active_doc().display_name.as_deref(), Some("Untitled 1"));
}

/// A prior error message must never block a Guard from being raised (plan
/// WP1: errors are a non-modal log entry now, orthogonal to the Guard slot
/// — unlike the pre-WP1 modal error banner, which used to outrank and refuse a
/// lower-priority Guard request).
#[test]
fn an_error_message_never_blocks_a_guard_from_being_raised() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');

    rune_tui::messages::error(&mut app, "boom");
    assert_eq!(rune_tui::messages::newest_text(&app), Some("boom"));

    workspace::request_close(&mut app, second, &mut Effects::default());

    assert!(
        matches!(
            app.guard,
            Some(ref prompt) if prompt.kind == rune_tui::guard::GuardKind::DirtyClose
        ),
        "a prior error message must never block a Guard from being raised"
    );
}

/// A cancellation ack must never cost the user an unacknowledged save
/// failure — the log is append-only, so an unrelated
/// cancellation posts its own entry without ever touching an earlier one.
#[test]
fn escape_on_a_guard_keeps_an_unacknowledged_save_failure_in_the_log() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');
    rune_tui::messages::error(&mut app, "save failed: disk full");

    workspace::request_close(&mut app, second, &mut Effects::default());
    assert!(app.guard.is_some(), "a dirty close arms the Guard");

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Escape)), &mut effects);

    assert!(app.guard.is_none(), "Escape still cancels the Guard");
    assert_eq!(
        rune_tui::messages::log_text(&app),
        "save failed: disk full\nclose cancelled",
        "the save failure must survive an unrelated cancellation, in order"
    );
}
