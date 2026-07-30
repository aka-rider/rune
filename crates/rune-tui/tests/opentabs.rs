//! WP5 "Done when" tests: Open Tabs rendering/switching, and the close
//! guard's three resolutions (`[S]ave`, `[D]iscard`, `Esc`), driven against
//! a `Mem` vfs seeded with two files.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App, StatusSource};
use rune_tui::commands::edit;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::testgrid;
use rune_tui::{opentabs, workspace};
use rune_vfs::{Mem, Vfs};

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn seeded_vfs() -> Arc<Mem> {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/a.md"), b"a content")
        .expect("seed a.md");
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    mem
}

/// An `App` with `/root/a.md` as the initial (sole) document, no store
/// bound (`db: None`) — so any save on any document funnels through the
/// no-store `Msg::SaveDone` fallback (Assumption A1), matching an
/// Explorer-opened document's own shape.
fn app_with(mem: &Arc<Mem>) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        Buffer::new("a content"),
        Some(PathBuf::from("/root/a.md")),
        vfs,
        None,
    );
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

/// Opens `/root/b.md` as a second document via the real `workspace::
/// open_path` — mirroring how a real session accumulates tabs.
fn open_second(app: &mut App) -> rune_tui::document::DocumentId {
    let first = app.active;
    workspace::open_path(app, Path::new("/root/b.md"));
    let second = app.active;
    assert_ne!(
        first, second,
        "test setup: b.md must open as a NEW document"
    );
    second
}

fn key(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

fn plain(code: KeyCode) -> KeyInput {
    key(code, Mods::NONE)
}

fn ctrl_w() -> KeyInput {
    key(
        KeyCode::Char('w'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    )
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
    app.focus = Pane::Tabs;
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
    app.focus = Pane::Tabs;
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
    app.focus = Pane::Editor;
    workspace::switch_to(&mut app, first); // back to a.md, cursor -> index 0

    app.tabs.nav.cursor = 1; // b.md's row
    let outcome = opentabs::handle_key(&mut app, plain(KeyCode::Enter));

    assert_eq!(outcome, rune_tui::keymap::KeyOutcome::Consumed);
    assert_eq!(app.active, second);
    assert_eq!(app.focus, Pane::Editor);
}

/// A dirty document's tab shows the `x` dirty marker; a clean one shows a
/// blank in its place (plan WP5.S1).
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
        text.contains(" x "),
        "expected the dirty marker somewhere in the tab rows:\n{text}"
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

    workspace::request_close(&mut app, second);
    assert!(app.modal.is_some(), "a dirty close must arm a modal");

    let before = app.doc(second).unwrap().buffer.content().to_string();
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Char('q'))), &mut effects);

    assert_eq!(
        app.doc(second).unwrap().buffer.content(),
        before,
        "a key consumed by the Guard must never reach commands::edit"
    );
    assert!(
        app.modal.is_some(),
        "an unbound key must leave the Guard up"
    );
    assert!(app.documents.contains_key(&second), "must not close yet");
}

/// `^w`, end to end through the real four-stage pipeline with the Tabs
/// pane focused, now resolves at the GLOBAL pipeline stage (WP4's
/// `GlobalCommand::CloseFile`): it requests closing `app.active`, not
/// whichever row the Tabs cursor happens to sit on — arming the Guard for
/// a dirty active document exactly like calling `workspace::request_close`
/// directly.
#[test]
fn ctrl_w_on_the_tabs_pane_requests_closing_the_active_document() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');
    assert_eq!(
        app.active, second,
        "test setup: b.md is the active document"
    );
    app.focus = Pane::Tabs;
    app.tabs.nav.cursor = 0; // a.md's row — deliberately NOT the active document

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl_w()), &mut effects);

    assert!(
        app.modal.is_some(),
        "^w on the dirty active document must arm the Guard, regardless of the Tabs cursor"
    );
    assert!(app.documents.contains_key(&second));
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

    workspace::request_close(&mut app, second);
    assert!(app.modal.is_some());
    // A degraded-save confirm gate armed for `second` and left unresolved
    // (review fix: `close_now` must sweep it, not just `pending_close_on_
    // save`) — must not survive the close as a dangling reference to a
    // document that no longer exists.
    app.pending_save_confirm = Some((second, 0));

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Char('d'))), &mut effects);

    assert!(app.modal.is_none());
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

    workspace::request_close(&mut app, second);
    assert!(app.modal.is_some());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Char('s'))), &mut effects);

    assert!(
        app.modal.is_none(),
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

    workspace::request_close(&mut app, second);
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

    workspace::request_close(&mut app, second);
    assert!(app.modal.is_some());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Escape)), &mut effects);

    assert!(app.modal.is_none());
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

    workspace::request_close(&mut app, second);
    assert!(app.modal.is_some());

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Escape)), &mut effects);

    assert_eq!(app.status_message.as_deref(), Some("close cancelled"));
    assert_eq!(app.status_source, StatusSource::Other);
}

/// Closing the last remaining document is refused outright — rune always
/// shows one document.
#[test]
fn closing_the_last_document_is_refused() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let only = app.active;
    assert_eq!(app.documents.len(), 1);

    workspace::request_close(&mut app, only);

    assert!(
        app.modal.is_none(),
        "must not arm a Guard for the last document"
    );
    assert!(app.documents.contains_key(&only));
    assert_eq!(app.documents.len(), 1);
    assert!(app.status_message.is_some());
}

/// A Guard armed while an Error banner is already up must not displace it
/// (plan Risks, "Banner reentrancy": Error always outranks Guard).
#[test]
fn a_guard_does_not_replace_an_existing_error_modal() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');

    rune_tui::banner::report_error(&mut app, "boom");
    assert!(matches!(app.modal, Some(rune_tui::banner::Modal::Error(_))));

    workspace::request_close(&mut app, second);

    assert!(
        matches!(app.modal, Some(rune_tui::banner::Modal::Error(_))),
        "the pre-existing Error modal must survive a lower-priority Guard request"
    );
}

/// A `^`-modified digit chord, e.g. `ctrl(&'1')` for `^1`.
fn ctrl(c: char) -> KeyInput {
    key(
        KeyCode::Char(c),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    )
}

/// `^w` from the EDITOR pane (WP4) closes the active clean document
/// straight away — no Guard, since there is nothing to lose.
#[test]
fn ctrl_w_from_editor_focus_closes_the_active_clean_document() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let first = app.active;
    let second = open_second(&mut app);
    assert_eq!(app.active, second);
    app.focus = Pane::Editor;

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl_w()), &mut effects);

    assert!(app.modal.is_none(), "a clean document closes immediately");
    assert!(!app.documents.contains_key(&second), "b.md must be closed");
    assert_eq!(app.active, first, "the sole remaining document takes over");
}

/// `^w` from the EDITOR pane on a DIRTY active document arms the Guard
/// instead of discarding it outright (WP4) — the same data-safety gate
/// `workspace::request_close` already gives every other close path.
#[test]
fn ctrl_w_from_editor_focus_on_a_dirty_document_arms_the_guard() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');
    assert!(app.doc(second).unwrap().is_dirty());
    app.focus = Pane::Editor;

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl_w()), &mut effects);

    match &app.modal {
        Some(rune_tui::banner::Modal::Guard(prompt)) => {
            assert_eq!(prompt.doc, second);
            assert_eq!(prompt.kind, rune_tui::banner::GuardKind::DirtyClose);
        }
        Some(_) => panic!("expected a DirtyClose Guard, got some other modal"),
        None => panic!("expected a DirtyClose Guard, got no modal"),
    }
    assert!(
        app.documents.contains_key(&second),
        "must not close before the Guard is resolved"
    );
}

/// `^1`, end to end through the real pipeline from the EDITOR pane (WP4),
/// jumps straight to the first tab.
#[test]
fn ctrl_1_switches_to_the_first_tab() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let first = app.active;
    open_second(&mut app);
    app.focus = Pane::Editor;

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl('1')), &mut effects);

    assert_eq!(app.active, app.tabs.order[0]);
    assert_eq!(app.active, first);
}

/// `^0` is the TENTH tab (WP4) — matching what the tab strip itself prints
/// for the first ten tabs (`(idx + 1) % 10`).
#[test]
fn ctrl_0_switches_to_the_tenth_tab() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    for i in 0..9 {
        app.open_document(Buffer::new(format!("doc {i}")));
    }
    assert_eq!(app.tabs.order.len(), 10);
    let tenth = app.tabs.order[9];
    let away = app.tabs.order[0];
    workspace::switch_to(&mut app, away); // away from the tenth
    app.focus = Pane::Editor;

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl('0')), &mut effects);

    assert_eq!(app.active, tenth);
    assert_eq!(app.active, app.tabs.order[9]);
}

/// The routing proof (WP4): `^1` fired from EXPLORER focus still switches
/// tabs. If `TabSwitch` were resolved by a pane-local table instead of
/// `GLOBAL_BINDINGS`, this would fail — the Explorer pane has no such
/// binding of its own.
#[test]
fn ctrl_1_from_explorer_focus_switches_tabs() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let first = app.active;
    open_second(&mut app);
    app.focus = Pane::Explorer;

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl('1')), &mut effects);

    assert_eq!(app.active, first, "^1 switched to the first tab");
    assert_eq!(app.active, app.tabs.order[0]);
}

/// A digit chord past the number of open tabs is a silent no-op (WP4) —
/// no panic, no change of `app.active`.
#[test]
fn an_out_of_range_tab_digit_is_a_no_op() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    open_second(&mut app);
    assert_eq!(app.tabs.order.len(), 2);
    let before = app.active;
    app.focus = Pane::Editor;

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(ctrl('9')), &mut effects);

    assert_eq!(
        app.active, before,
        "^9 with only 2 tabs open must be a no-op"
    );
}

/// A cancellation ack must never cost the user an unacknowledged save
/// failure. The footer ranks a `SaveError` above ordinary status precisely
/// because it is the user's only notice that their bytes did not reach
/// disk; cancelling an unrelated Guard is the least important thing the
/// status row can say, so it yields rather than overwriting.
#[test]
fn escape_on_a_guard_does_not_clobber_an_unacknowledged_save_failure() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let second = open_second(&mut app);
    edit::insert_char(&mut app, second, '!');
    app.set_status("save failed: disk full", StatusSource::SaveError);

    workspace::request_close(&mut app, second);
    assert!(app.modal.is_some(), "a dirty close arms the Guard");

    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(plain(KeyCode::Escape)), &mut effects);

    assert!(app.modal.is_none(), "Escape still cancels the Guard");
    assert_eq!(
        app.status_message.as_deref(),
        Some("save failed: disk full"),
        "the save failure must survive an unrelated cancellation"
    );
    assert_eq!(app.status_source, StatusSource::SaveError);
}
