//! Copy coverage for the message-log pane, split out of `messages.rs` to
//! stay under the source-file line budget: the pane's own copy chord
//! matching the editor's `EDITOR_BINDINGS`, plus mouse drag-selection,
//! click-to-focus, and copy-on-release — all routed through the same
//! capped OSC-52 path every other copy in the app uses. State/keyboard/
//! timer coverage for the pane lives in the sibling `messages.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod messages_common;

use ratatui::layout::Rect;

use rune_core::cursor::{Cursor, CursorId, CursorSet};
use rune_fuzz::Session;
use rune_tui::app;
use rune_tui::clipboard::{OSC52_MAX_PAYLOAD_BYTES, osc52_copy};
use rune_tui::keymap::KeyInput;
use rune_tui::layout;
use rune_tui::messages;
use rune_tui::pane::Pane;
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{Effects, Msg};

use messages_common::{app_for, ctrl_e, super_c};

/// Finding 3: the pane's copy chord must be resolved through the SAME
/// binding table the editor's own `Copy` row declares (`EDITOR_BINDINGS`),
/// not a second, hand-written copy of the chord shape — so a future rebind
/// of `Copy` can never desync the two. Every chord derived from the table
/// (not hardcoded here) must trigger a copy while the pane is focused with
/// a selection.
#[test]
fn the_panes_copy_key_matches_every_editor_copy_binding() {
    use rune_tui::binding::KeyMatch;
    use rune_tui::keymap::Command;
    use rune_tui::keymap::editor_bindings::EDITOR_BINDINGS;

    let copy_bindings: Vec<_> = EDITOR_BINDINGS
        .iter()
        .filter(|b| b.cmd == Command::Copy)
        .collect();
    assert!(
        !copy_bindings.is_empty(),
        "test setup: EDITOR_BINDINGS must declare at least one Copy row"
    );

    for binding in copy_bindings {
        let pattern = binding.key;
        let KeyMatch::Code(code) = pattern.key else {
            panic!("Copy's binding is not a plain key code");
        };

        let mut session = app_for("hello");
        messages::warn(session.app_mut(), "hello world");
        session.app_mut().sync_view();

        let mut focus_effects = Effects::default();
        app::update(session.app_mut(), ctrl_e(), &mut focus_effects);
        assert_eq!(
            session.app().focus(),
            Pane::Messages,
            "test setup: pane must be focused"
        );
        let content = messages::doc(session.app()).buffer.content().to_string();
        let start = content
            .find("hello world")
            .expect("test setup: log document must contain the posted text");
        let end = start + "hello world".len();
        messages::doc_mut(session.app_mut()).cursors = CursorSet::new_from(&[Cursor {
            position: end,
            anchor: start,
            desired_col: 0,
            id: CursorId::FIRST,
        }]);

        let mut effects = Effects::default();
        app::update(
            session.app_mut(),
            Msg::Key(KeyInput {
                code,
                mods: pattern.mods,
            }),
            &mut effects,
        );

        assert_eq!(
            effects.raw,
            vec![osc52_copy(b"hello world")],
            "the pane must respond to the {pattern:?} chord EDITOR_BINDINGS \
             declares for Copy"
        );
    }
}

/// The pane's own `Rect` this frame — panics (test-only) if it's closed,
/// since every test here opens it first.
fn pane_rect(session: &Session) -> Rect {
    let app = session.app();
    let area = Rect::new(0, 0, app.frame_width, app.frame_height);
    layout::geometry(area, app)
        .messages
        .expect("test setup: the pane must be open")
}

/// Sends one raw mouse event through the real `update`, returning the
/// `Effects` it produced — mirrors `splitter_drag.rs`'s own `send` helper,
/// except this one hands back `Effects` (these tests assert on
/// `effects.raw`, the OSC-52 clipboard write) instead of resyncing for a
/// later geometry read.
fn mouse(session: &mut Session, kind: MouseKind, column: u16, row: u16) -> Effects {
    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Mouse(MouseInput {
            kind,
            column,
            row,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );
    effects
}

/// A `Warn` glyph (`"! "`) is pure ASCII, unlike `Info`'s middle dot — so a
/// warning's rendered row has a exactly-one-byte-per-cell mapping from the
/// glyph prefix all the way through the message text, and a drag's target
/// columns can be reasoned about directly instead of guessing at a
/// multi-byte glyph's cell width.
const WARN_PREFIX_COLS: u16 = 2; // "! "

/// A plain drag inside the pane selects text and, on release, copies
/// exactly the dragged range through the same capped OSC-52 path every
/// other copy in the app uses.
#[test]
fn dragging_inside_the_pane_selects_and_copies_on_release() {
    let mut session = app_for("hello");
    messages::warn(session.app_mut(), "hello world");
    session.app_mut().sync_view();

    let rect = pane_rect(&session);
    let row = rect.y + 1; // first content row, past the separator
    let start_col = rect.x + WARN_PREFIX_COLS; // "hello" starts here
    let end_col = rect.x + 40; // past the row's content — clamps to its end

    mouse(
        &mut session,
        MouseKind::Down(MouseButton::Left),
        start_col,
        row,
    );
    mouse(
        &mut session,
        MouseKind::Drag(MouseButton::Left),
        end_col,
        row,
    );
    let effects = mouse(&mut session, MouseKind::Up(MouseButton::Left), end_col, row);

    assert_eq!(
        effects.raw,
        vec![osc52_copy(b"hello world")],
        "release must copy exactly the dragged selection"
    );
}

/// A drag begun in the pane must still copy — and must clear its latch —
/// even when the button comes up somewhere else entirely (mode 1002
/// reports no hover, so a lost release has no second signal to recover
/// from).
#[test]
fn a_drag_released_outside_the_pane_still_clears_the_drag_and_still_copies() {
    let mut session = app_for("hello");
    messages::warn(session.app_mut(), "hello world");
    session.app_mut().sync_view();

    let rect = pane_rect(&session);
    let row = rect.y + 1;
    let start_col = rect.x + WARN_PREFIX_COLS;
    let end_col = rect.x + 40;

    mouse(
        &mut session,
        MouseKind::Down(MouseButton::Left),
        start_col,
        row,
    );
    mouse(
        &mut session,
        MouseKind::Drag(MouseButton::Left),
        end_col,
        row,
    );
    // Released far above the pane — outside every rect in the frame.
    let effects = mouse(&mut session, MouseKind::Up(MouseButton::Left), 0, 0);

    assert_eq!(
        effects.raw,
        vec![osc52_copy(b"hello world")],
        "a release outside the pane must still copy the drag's selection"
    );

    // The latch must be cleared too: a second, unrelated Up must not copy
    // again.
    let effects_again = mouse(&mut session, MouseKind::Up(MouseButton::Left), 0, 0);
    assert!(
        effects_again.raw.is_empty(),
        "the drag must be cleared after its first release"
    );
}

/// A drag inside the EDITOR must never reach the log document's own
/// cursor — proven by focusing the pane afterward and asking it to copy,
/// which must find no selection to copy at all.
#[test]
fn a_drag_in_the_editor_never_touches_the_log_documents_cursor() {
    let mut session = app_for("hello world\n");
    messages::warn(session.app_mut(), "hello world");
    session.app_mut().sync_view();

    let area = Rect::new(0, 0, session.app().frame_width, session.app().frame_height);
    let editor = layout::geometry(area, session.app()).editor;
    mouse(
        &mut session,
        MouseKind::Down(MouseButton::Left),
        editor.x,
        editor.y,
    );
    mouse(
        &mut session,
        MouseKind::Drag(MouseButton::Left),
        editor.x + 5,
        editor.y,
    );
    mouse(
        &mut session,
        MouseKind::Up(MouseButton::Left),
        editor.x + 5,
        editor.y,
    );

    let mut effects = Effects::default();
    app::update(session.app_mut(), ctrl_e(), &mut effects); // focus the pane
    let mut copy_effects = Effects::default();
    app::update(session.app_mut(), super_c(), &mut copy_effects);

    assert!(
        copy_effects.raw.is_empty(),
        "an editor drag must never leave a selection on the log document"
    );
}

/// A click inside the pane must never move the active editor document's
/// own caret.
#[test]
fn a_click_in_the_pane_does_not_move_the_editor_caret() {
    let mut session = app_for("hello world\n");
    messages::warn(session.app_mut(), "hello world");
    session.app_mut().sync_view();
    let before = session.app().active_doc().cursors.primary().position;

    let rect = pane_rect(&session);
    mouse(
        &mut session,
        MouseKind::Down(MouseButton::Left),
        rect.x + WARN_PREFIX_COLS,
        rect.y + 1,
    );

    assert_eq!(
        session.app().active_doc().cursors.primary().position,
        before,
        "a click inside the pane must never move the active editor document's caret"
    );
}

/// A selection over `OSC52_MAX_PAYLOAD_BYTES` must post an error message
/// instead of writing a raw sequence a terminal multiplexer would just
/// drop — triple-click selects the whole (wrapped) paragraph in one
/// gesture, mirroring the editor's own whole-logical-line triple-click.
#[test]
fn an_over_cap_selection_posts_an_error_instead_of_writing_raw() {
    let mut session = app_for("hello");
    let huge = "x".repeat(OSC52_MAX_PAYLOAD_BYTES + 1);
    messages::warn(session.app_mut(), huge);
    session.app_mut().sync_view();

    let rect = pane_rect(&session);
    let row = rect.y + 1;
    let col = rect.x + WARN_PREFIX_COLS;

    mouse(&mut session, MouseKind::Down(MouseButton::Left), col, row);
    mouse(&mut session, MouseKind::Down(MouseButton::Left), col, row);
    mouse(&mut session, MouseKind::Down(MouseButton::Left), col, row); // triple click
    let release = mouse(&mut session, MouseKind::Up(MouseButton::Left), col, row);

    assert!(
        release.raw.is_empty(),
        "an over-cap selection must never reach the OSC-52 raw output"
    );
    assert!(
        messages::newest_text(session.app()).is_some_and(|t| t.contains("too large")),
        "an over-cap copy must post a message reporting the failure"
    );
}
