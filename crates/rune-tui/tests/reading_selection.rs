//! A mouse selection made inside a read-only document must be visible on
//! screen, even though such a document paints no caret. Every case drives
//! real `Msg::Key`/`Msg::Mouse` through `app::update` and reads STYLE back
//! off a rendered `TestBackend` buffer rather than text: the selection
//! background — not the copied bytes, which always worked — is what a
//! read-only document was silently failing to show.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use ratatui::buffer::Buffer as RtBuffer;

use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::document::ReadOnly;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::pointer::{ManualClock, MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

mod tui_render_common;
use tui_render_common::{HEIGHT, WIDTH, caret_column, render_to_test_backend};

fn send(app: &mut App, msg: Msg) {
    let mut effects = Effects::default();
    rune_tui::app::update(app, msg, &mut effects);
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

/// Builds an app already sized for `render_to_test_backend`'s `WIDTH`x
/// `HEIGHT` backend — `app.frame_width`/`frame_height` (not just the active
/// document's viewport) must match it, since `commands::mouse::handle`
/// hit-tests a `Msg::Mouse`'s column/row against `app.frame_width`/
/// `frame_height` itself, the same dimensions a click's caller renders
/// into.
fn app_sized(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.pointer_clock = Box::new(ManualClock::new());
    app.frame_width = WIDTH;
    app.frame_height = HEIGHT;
    app.sync_view();
    app
}

/// Translates an editor-relative `(col, row)` to the absolute frame
/// coordinates a real `MouseInput` carries — the same `layout::geometry`
/// call `commands::mouse::handle` itself uses, so a test can never
/// silently click the border/title row instead of editor content.
fn editor_origin(app: &App) -> (u16, u16) {
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let editor = rune_tui::layout::geometry(area, app).editor;
    (editor.x, editor.y)
}

fn drag_select(app: &mut App, down: (u16, u16), drag: (u16, u16)) {
    let (ox, oy) = editor_origin(app);
    send(
        app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::Down(MouseButton::Left),
            column: ox + down.0,
            row: oy + down.1,
            shift: false,
            alt: false,
            ctrl: false,
        }),
    );
    send(
        app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::Drag(MouseButton::Left),
            column: ox + drag.0,
            row: oy + drag.1,
            shift: false,
            alt: false,
            ctrl: false,
        }),
    );
}

/// The absolute backend column of any cell on row `y` painted with the
/// theme's selection background, or `None` if no cell on that row carries
/// it.
fn selection_column(
    buf: &RtBuffer,
    y: u16,
    width: u16,
    selection_bg: ratatui::style::Color,
) -> Option<u16> {
    (0..width).find(|&x| buf.cell((x, y)).is_some_and(|c| c.bg == selection_bg))
}

/// THE REGRESSION PIN. F1 opens the Help tab — a document that is
/// `ReadOnly::Always` — whose very first content row renders the `# Help`
/// heading as `◉ Help` (icon-decorated, plain text otherwise). Dragging
/// across the word "Help" must paint the selection background on it: before
/// this change, `Document::has_insertion_point` (`focused && !read_only`)
/// gated the selection highlight too, so a read-only document's drag
/// produced a real, copyable selection with NOTHING visible for it.
#[test]
fn a_mouse_selection_is_painted_in_a_read_only_document() {
    let mut app = app_sized("hello");
    send(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::F1,
            mods: Mods::NONE,
        }),
    );
    app.sync_view();
    assert_eq!(
        app.active_doc().read_only,
        ReadOnly::Always,
        "F1 must land on the read-only Help document"
    );

    // Row 0 of the editor content is `◉ Help`: icon at column 1, "Help"
    // starting at column 3. Dragging well past the word's last column
    // guarantees the whole word falls inside `[anchor, cursor)`.
    drag_select(&mut app, (3, 0), (10, 0));
    app.sync_view();

    let buf = render_to_test_backend(&app);
    let selection_bg = app.theme.chrome.selection_bg;
    let (_, oy) = editor_origin(&app);

    assert_eq!(
        selection_column(&buf, oy, WIDTH, selection_bg),
        Some(3 + editor_origin(&app).0),
        "the selection background must be visible on the Help document's own text"
    );
}

/// The same read-only selection as above, but pinning the OTHER half of the
/// split gate: the caret must stay invisible everywhere on screen, even
/// while the selection it belongs to IS visible.
#[test]
fn a_read_only_document_still_paints_no_caret_while_a_selection_is_visible() {
    let mut app = app_sized("hello");
    send(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::F1,
            mods: Mods::NONE,
        }),
    );
    app.sync_view();

    drag_select(&mut app, (3, 0), (10, 0));
    app.sync_view();

    let buf = render_to_test_backend(&app);
    for y in 0..HEIGHT {
        assert_eq!(
            caret_column(&buf, y, WIDTH),
            None,
            "row {y} painted a caret in a read-only document"
        );
    }
}

/// An unfocused document — even a read-only one holding a real selection —
/// paints neither overlay: `Document::shows_selection` is `self.focused`,
/// and moving focus off the editor (⌃B, `FocusExplorer`) drives that flag
/// false on the next `sync_view`, same as it already does for the caret.
#[test]
fn an_unfocused_read_only_document_paints_no_selection() {
    let mut app = app_sized("hello");
    send(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::F1,
            mods: Mods::NONE,
        }),
    );
    app.sync_view();

    drag_select(&mut app, (3, 0), (10, 0));
    app.sync_view();
    assert!(
        app.active_doc().cursors.primary().has_selection(),
        "the drag must have produced a real selection to make this test meaningful"
    );

    send(&mut app, ctrl('b')); // GlobalCommand::FocusExplorer
    assert_eq!(app.focus(), Pane::Explorer);
    app.sync_view();
    assert!(
        !app.active_doc().focused,
        "moving focus off the editor must clear the active document's focused flag"
    );

    let buf = render_to_test_backend(&app);
    let selection_bg = app.theme.chrome.selection_bg;
    for y in 0..HEIGHT {
        assert_eq!(
            selection_column(&buf, y, WIDTH, selection_bg),
            None,
            "row {y} painted a selection while the document was unfocused"
        );
    }
}

/// Pins that this split changed nothing for the common case: an ordinary
/// (editable, `ReadOnly::No`) document still paints BOTH the caret and the
/// selection background together, exactly as before.
#[test]
fn an_ordinary_documents_selection_is_unchanged() {
    let mut app = app_sized("hello world\n");

    drag_select(&mut app, (0, 0), (5, 0));
    app.sync_view();

    let buf = render_to_test_backend(&app);
    let selection_bg = app.theme.chrome.selection_bg;
    let (ox, oy) = editor_origin(&app);

    assert_eq!(
        selection_column(&buf, oy, WIDTH, selection_bg),
        Some(ox),
        "an ordinary document's drag selection must still paint its background"
    );
    assert_eq!(
        caret_column(&buf, oy, WIDTH),
        Some(ox + 5),
        "an ordinary document's caret must still be painted at the drag's end"
    );
}
