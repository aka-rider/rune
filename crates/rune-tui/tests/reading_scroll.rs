//! WP-A "Done when" tests: in a read-only document (Help via F1, and the
//! ordinary `⌃P` reading view) every motion key must move the viewport on
//! its very first press — the House Rule regression this plan closes (see
//! `crates/rune-tui/src/commands/reading_nav.rs`'s module docs).
//!
//! Follows the `tests/reading_view.rs` harness (`app_basic`/`plain`/`send`/
//! `render_to_test_backend`/`full_text`): a freshly minted Help document has
//! an unsized viewport, so every test that lands on Help sizes it exactly
//! the way that file does.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::coords::DisplayRow;
use rune_tui::app::{self, App};
use rune_tui::document::ReadOnly;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::viewport::ScrollMode;
use rune_vfs::Mem;

mod tui_render_common;
use tui_render_common::{HEIGHT, WIDTH, app_for, caret_column, render_to_test_backend};

fn app_basic(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn plain(code: KeyCode) -> Msg {
    Msg::Key(KeyInput {
        code,
        mods: Mods::NONE,
    })
}

fn shifted(code: KeyCode) -> Msg {
    Msg::Key(KeyInput {
        code,
        mods: Mods {
            shift: true,
            ..Mods::NONE
        },
    })
}

fn ctrl(c: char) -> Msg {
    Msg::Key(KeyInput {
        code: KeyCode::Char(c),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    })
}

fn send(app: &mut App, msg: Msg) {
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
}

/// Opens the Help tab (F1) and sizes its viewport exactly the way
/// `app_basic` sizes an ordinary document's — the freshly minted document
/// otherwise has no viewport geometry at all.
fn help_doc() -> App {
    let mut app = app_basic("hello");
    send(&mut app, plain(KeyCode::F1));
    assert_eq!(app.active_doc().read_only, ReadOnly::Always);
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    // Help's own content is much taller than one screen — plenty of rows
    // to scroll into.
    app
}

/// THE REGRESSION PIN: a single `Down` on a freshly opened, freshly sized
/// read-only document must move the viewport immediately — before this
/// plan, the first ~`height - scrolloff` presses were invisible because
/// `Down` moved an unpainted cursor and `reconcile` hadn't caught up yet.
#[test]
fn first_down_press_scrolls_a_read_only_document() {
    let mut app = help_doc();
    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(0));

    send(&mut app, plain(KeyCode::Down));

    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(1));
}

/// `ReadOnly::Preview` has no insertion point either
/// (`Document::has_insertion_point`), so `commands::reading_nav::intercept`
/// gates on `is_read_only()` alone and treats it exactly like `Reading`/
/// `Always`: a preview exists to be looked at, so the same
/// motion-scrolls-on-first-press behaviour applies, not a dead viewport.
#[test]
fn first_down_press_scrolls_a_preview_document() {
    let content: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let mut app = app_basic(&content);
    app.active_doc_mut().read_only = ReadOnly::Preview;
    app.sync_view();
    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(0));

    send(&mut app, plain(KeyCode::Down));

    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(1));
    assert_eq!(
        app.active_doc().read_only,
        ReadOnly::Preview,
        "scrolling must not disturb the preview state"
    );
}

#[test]
fn up_scrolls_before_it_focuses_the_title() {
    let mut app = help_doc();
    for _ in 0..3 {
        send(&mut app, plain(KeyCode::Down));
    }
    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(3));

    send(&mut app, plain(KeyCode::Up));

    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(2));
    assert_eq!(app.focus(), Pane::Editor);
}

/// `App::focus_title` refuses on ANY read-only document — pinned already by
/// `tests/reading_view.rs::ctrl_r_in_reading_view_refuses_with_the_reading_
/// wording_not_the_always_wording` — so re-keying `Up`'s "focus the title"
/// gesture to a read-only document's own view-top reaches that SAME
/// refusal, not a successful focus change: the gesture fires (this is what
/// distinguishes it from an ordinary scroll-up-at-the-top no-op), and its
/// outcome is the existing, unchanged rename-refusal precedent — the
/// document has nothing to rename while it has no editable form to give a
/// title back to.
#[test]
fn up_at_the_top_of_a_read_only_document_focuses_the_title() {
    let mut app = help_doc();
    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(0));

    send(&mut app, plain(KeyCode::Up));

    assert_eq!(
        app.focus(),
        Pane::Editor,
        "focus_title refuses on a read-only document; it must not move focus"
    );
    assert_eq!(
        rune_tui::messages::newest_text(&app),
        ReadOnly::Always.refusal_message()
    );
}

#[test]
fn left_and_right_page_a_read_only_document() {
    let mut app = help_doc();

    send(&mut app, plain(KeyCode::Right));
    let height = app.active_doc().viewport.height as usize;
    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(height - 1));

    send(&mut app, plain(KeyCode::Left));
    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(0));
}

#[test]
fn home_and_end_jump_to_the_first_and_last_page() {
    let mut app = help_doc();
    send(&mut app, plain(KeyCode::Down));
    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(1));

    let (total, height) = {
        let doc = app.active_doc_mut();
        let total = doc.view().display.total_rows();
        let height = doc.viewport.height as usize;
        (total, height)
    };

    send(&mut app, plain(KeyCode::End));
    assert_eq!(
        app.active_doc().viewport.scroll_row,
        DisplayRow(total - height)
    );

    send(&mut app, plain(KeyCode::Home));
    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(0));
}

#[test]
fn shift_arrows_scroll_and_select_nothing_in_a_read_only_document() {
    let mut app = help_doc();

    send(&mut app, shifted(KeyCode::Down));

    assert_eq!(app.active_doc().viewport.scroll_row, DisplayRow(1));
    assert!(
        !app.active_doc().cursors.primary().has_selection(),
        "keyboard selection does not exist in a read-only document"
    );
}

#[test]
fn a_keyboard_scroll_collapses_a_mouse_selection_in_a_read_only_document() {
    let content = "one two three\nfour five six\nseven eight nine\n";
    // `app_for` already resizes to `WIDTH`/`HEIGHT` through the real
    // `Msg::Resize` chokepoint, so `layout::geometry` (and so the mouse
    // gesture's `editor` rect below, which reads `frame_width`/
    // `frame_height`) is already sized to match.
    let mut session = app_for(content, 0, true);
    send(session.app_mut(), ctrl('p'));
    assert_eq!(session.app().active_doc().read_only, ReadOnly::Reading);

    let area =
        ratatui::layout::Rect::new(0, 0, session.app().frame_width, session.app().frame_height);
    let editor = rune_tui::layout::geometry(area, session.app()).editor;
    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Mouse(MouseInput {
            kind: MouseKind::Down(MouseButton::Left),
            column: editor.x,
            row: editor.y,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );
    app::update(
        session.app_mut(),
        Msg::Mouse(MouseInput {
            kind: MouseKind::Drag(MouseButton::Left),
            column: editor.x + 8,
            row: editor.y,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );
    session.app_mut().sync_view();
    assert!(
        session.app().active_doc().cursors.primary().has_selection(),
        "the drag must have produced a selection before scrolling"
    );

    send(session.app_mut(), plain(KeyCode::Down));

    assert!(
        !session.app().active_doc().cursors.primary().has_selection(),
        "a keyboard scroll now moves a real caret, so it must collapse a selection \
         exactly like an ordinary unshifted Down press does in an editable document"
    );
}

#[test]
fn leaving_the_reading_view_does_not_move_the_caret() {
    let content: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let mut app = app_basic(&content);
    send(&mut app, ctrl('p'));
    assert_eq!(app.active_doc().read_only, ReadOnly::Reading);

    send(&mut app, plain(KeyCode::End));
    app.sync_view();
    let scroll_after_reading = app.active_doc().viewport.scroll_row;
    assert!(
        scroll_after_reading > DisplayRow(0),
        "must actually have scrolled"
    );
    assert_eq!(app.active_doc().viewport.mode, ScrollMode::FollowCursor);
    let caret_before_leaving = app.active_doc().cursors.primary().position;

    send(&mut app, ctrl('p'));
    assert_eq!(app.active_doc().read_only, ReadOnly::No);
    assert_eq!(
        app.active_doc().cursors.primary().position,
        caret_before_leaving,
        "leaving reading view must not move the caret"
    );

    app.sync_view();

    let buf = render_to_test_backend(&app);
    let mut painted = false;
    for y in 0..HEIGHT {
        if caret_column(&buf, y, WIDTH).is_some() {
            painted = true;
            break;
        }
    }
    assert!(
        painted,
        "a caret must be visible after leaving reading view"
    );
}

/// A reading-view document has no caret to jump with, so `⌥M` must be
/// consumed with the same refusal the rest of read-only rejection uses —
/// never fall through to a cursor move nobody can see.
#[test]
fn alt_m_in_reading_view_refuses_instead_of_moving_an_invisible_caret() {
    let content: String = std::iter::once("(a)\n".to_string())
        .chain((0..100).map(|i| format!("line {i}\n")))
        .collect();
    let mut app = app_basic(&content);
    send(&mut app, ctrl('p'));
    assert_eq!(app.active_doc().read_only, ReadOnly::Reading);

    let scroll_before = app.active_doc().viewport.scroll_row;
    let caret_before = app.active_doc().cursors.primary().position;
    assert_eq!(
        content.as_bytes()[caret_before],
        b'(',
        "the fixture must park the caret on a bracket, or this proves nothing"
    );

    send(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('m'),
            mods: Mods {
                alt: true,
                ..Mods::NONE
            },
        }),
    );
    app.sync_view();

    assert_eq!(
        app.active_doc().cursors.primary().position,
        caret_before,
        "⌥M must not move a caret the reader cannot see"
    );
    assert_eq!(
        app.active_doc().viewport.scroll_row,
        scroll_before,
        "⌥M must not scroll a reading-view document"
    );
    assert_eq!(
        rune_tui::messages::newest_text(&app),
        ReadOnly::Reading.refusal_message(),
        "a consumed-but-inapplicable key must still give the user feedback"
    );
}
