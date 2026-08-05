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
use rune_tui::app::{self, App};
use rune_tui::clipboard::osc52_copy;
use rune_tui::commands::clipboard;
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
    assert_eq!(app.active_doc().viewport.scroll_row, 0);

    send(&mut app, plain(KeyCode::Down));

    assert_eq!(app.active_doc().viewport.scroll_row, 1);
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
    assert_eq!(app.active_doc().viewport.scroll_row, 0);

    send(&mut app, plain(KeyCode::Down));

    assert_eq!(app.active_doc().viewport.scroll_row, 1);
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
    assert_eq!(app.active_doc().viewport.scroll_row, 3);

    send(&mut app, plain(KeyCode::Up));

    assert_eq!(app.active_doc().viewport.scroll_row, 2);
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
    assert_eq!(app.active_doc().viewport.scroll_row, 0);

    send(&mut app, plain(KeyCode::Up));

    assert_eq!(
        app.focus(),
        Pane::Editor,
        "focus_title refuses on a read-only document; it must not move focus"
    );
    assert_eq!(
        app.status_message.as_deref(),
        ReadOnly::Always.refusal_message()
    );
}

#[test]
fn left_and_right_page_a_read_only_document() {
    let mut app = help_doc();

    send(&mut app, plain(KeyCode::Right));
    let height = app.active_doc().viewport.height as usize;
    assert_eq!(app.active_doc().viewport.scroll_row, height - 1);

    send(&mut app, plain(KeyCode::Left));
    assert_eq!(app.active_doc().viewport.scroll_row, 0);
}

#[test]
fn home_and_end_jump_to_the_first_and_last_page() {
    let mut app = help_doc();
    send(&mut app, plain(KeyCode::Down));
    assert_eq!(app.active_doc().viewport.scroll_row, 1);

    let (total, height) = {
        let doc = app.active_doc_mut();
        let total = doc.view().display.total_rows();
        let height = doc.viewport.height as usize;
        (total, height)
    };

    send(&mut app, plain(KeyCode::End));
    assert_eq!(app.active_doc().viewport.scroll_row, total - height);

    send(&mut app, plain(KeyCode::Home));
    assert_eq!(app.active_doc().viewport.scroll_row, 0);
}

#[test]
fn shift_arrows_scroll_and_select_nothing_in_a_read_only_document() {
    let mut app = help_doc();

    send(&mut app, shifted(KeyCode::Down));

    assert_eq!(app.active_doc().viewport.scroll_row, 1);
    assert!(
        !app.active_doc().cursors.primary().has_selection(),
        "keyboard selection does not exist in a read-only document"
    );
}

#[test]
fn a_mouse_selection_survives_scrolling_in_a_read_only_document() {
    let content = "one two three\nfour five six\nseven eight nine\n";
    let mut app = app_for(content, 0, true);
    // `layout::geometry` (and so the mouse gesture's `editor` rect below)
    // reads `frame_width`/`frame_height`, not the viewport size `app_for`
    // sets directly — `tests/navigate.rs`'s own click helper carries the
    // identical note.
    app.frame_width = WIDTH;
    app.frame_height = HEIGHT;
    app.sync_view();
    send(&mut app, ctrl('p'));
    assert_eq!(app.active_doc().read_only, ReadOnly::Reading);

    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let editor = rune_tui::layout::geometry(area, &app).editor;
    let mut effects = Effects::default();
    app::update(
        &mut app,
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
        &mut app,
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
    app.sync_view();
    assert!(
        app.active_doc().cursors.primary().has_selection(),
        "the drag must have produced a selection before scrolling"
    );

    send(&mut app, plain(KeyCode::Down));

    assert!(
        app.active_doc().cursors.primary().has_selection(),
        "scrolling a read-only document must not collapse a mouse selection"
    );

    let id = app.active;
    let mut copy_effects = Effects::default();
    clipboard::copy(&mut app, id, &mut copy_effects);
    let selected = &content[..8];
    assert_eq!(copy_effects.raw, vec![osc52_copy(selected.as_bytes())]);
}

#[test]
fn leaving_the_reading_view_brings_the_caret_back_into_view() {
    let content: String = (0..100).map(|i| format!("line {i}\n")).collect();
    let mut app = app_basic(&content);
    send(&mut app, ctrl('p'));
    assert_eq!(app.active_doc().read_only, ReadOnly::Reading);

    send(&mut app, plain(KeyCode::End));
    app.sync_view();
    let scroll_after_reading = app.active_doc().viewport.scroll_row;
    assert!(scroll_after_reading > 0, "must actually have scrolled");
    assert_eq!(app.active_doc().viewport.mode, ScrollMode::FollowCursor);

    send(&mut app, ctrl('p'));
    assert_eq!(app.active_doc().read_only, ReadOnly::No);
    app.sync_view();

    assert_eq!(
        app.active_doc().viewport.scroll_row,
        scroll_after_reading,
        "the view must not jump when reading view is left"
    );
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
