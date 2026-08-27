#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::layout::{self, Geometry};
use rune_tui::pane::Pane;
use rune_tui::pointer::{ManualClock, MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs, VfsTestExt};

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn seeded_vfs(files: usize) -> Arc<Mem> {
    let mem = Arc::new(Mem::new());
    for i in 0..files {
        let path = format!("/root/f{i:03}.md");
        mem.save_atomic(Path::new(&path), b"content")
            .expect("seed file");
    }
    mem.save_atomic(Path::new("/root/a.md"), b"a content")
        .expect("seed a.md");
    mem.save_atomic(Path::new("/root/b.md"), b"b content")
        .expect("seed b.md");
    mem
}

fn app_with(mem: &Arc<Mem>) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        Buffer::new("a content\nsecond line\nthird line\n"),
        Some(PathBuf::from("/root/a.md")),
        vfs,
        None,
    );
    app.clock = Arc::new(ManualClock::new());
    app.frame = Some(rune_tui::app::FrameSize::new(WIDTH, HEIGHT));
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn ctrl_b() -> Msg {
    Msg::Key(KeyInput {
        code: KeyCode::Char('b'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    })
}

fn ctrl_shift_f() -> Msg {
    Msg::Key(KeyInput {
        code: KeyCode::Char('F'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    })
}

fn ctrl_e() -> Msg {
    Msg::Key(KeyInput {
        code: KeyCode::Char('e'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    })
}

fn run_cmds(app: &mut App, mut effects: Effects) {
    for _ in 0..32 {
        if effects.cmds.is_empty() {
            break;
        }
        let cmd = effects.cmds.remove(0);
        let Some(msg) = cmd.run() else {
            continue;
        };
        let mut reply = Effects::default();
        app::update(app, msg, &mut reply);
        effects.cmds.append(&mut reply.cmds);
    }
    app.sync_view();
}

fn open_file_finder(app: &mut App) {
    let mut effects = Effects::default();
    app::update(app, ctrl_shift_f(), &mut effects);
    run_cmds(app, effects);
    assert!(app.filesearch().is_some(), "test setup: the finder is open");
}

fn show_left_column(app: &mut App) {
    let mut effects = Effects::default();
    app::update(app, ctrl_b(), &mut effects);
    assert_eq!(effects.cmds.len(), 1, "^b must enqueue exactly one Cmd");
    assert_eq!(effects.cmds[0].kind(), CmdKind::ReadDir);
    let cmd = effects.cmds.remove(0);
    let msg = cmd.run().expect("ReadDir Cmd replies with a Msg");
    let mut reply_effects = Effects::default();
    app::update(app, msg, &mut reply_effects);
    app.sync_view();
}

fn geometry(app: &App) -> Geometry {
    layout::geometry(app.frame_area(), app)
}

fn mouse(app: &mut App, kind: MouseKind, column: u16, row: u16) {
    let mut effects = Effects::default();
    app::update(
        app,
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
    app.sync_view();
}

fn click(app: &mut App, column: u16, row: u16) {
    mouse(app, MouseKind::Down(MouseButton::Left), column, row);
}

fn click_editor_origin(app: &mut App) {
    let editor = geometry(app).editor;
    click(app, editor.x, editor.y);
}

#[test]
fn a_click_in_the_editor_takes_focus_from_the_explorer() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    show_left_column(&mut app);
    assert_eq!(app.focus(), Pane::Explorer, "test setup: explorer focused");

    click_editor_origin(&mut app);
    assert_eq!(app.focus(), Pane::Editor);
}

#[test]
fn a_click_in_the_messages_pane_focuses_it() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    let mut effects = Effects::default();
    app::update(&mut app, ctrl_e(), &mut effects);
    app.sync_view();

    let pane = geometry(&app)
        .messages
        .expect("test setup: the messages pane is open");
    click_editor_origin(&mut app);
    assert_eq!(app.focus(), Pane::Editor, "test setup: editor focused");

    click(&mut app, pane.x + 1, pane.y);

    assert_eq!(app.focus(), Pane::Messages);
}

#[test]
fn a_click_on_an_explorer_row_focuses_it_and_selects_that_row() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    show_left_column(&mut app);
    click_editor_origin(&mut app);
    assert_eq!(app.focus(), Pane::Editor, "test setup: editor focused");

    let explorer = geometry(&app).explorer_inner;
    assert_eq!(app.explorer.nav.top, 0, "test setup: unscrolled explorer");
    click(&mut app, explorer.x + 1, explorer.y + 2);

    assert_eq!(app.focus(), Pane::Explorer);
    assert_eq!(app.explorer.nav.cursor, 1);
}

#[test]
fn a_click_on_the_explorer_root_row_focuses_without_moving_the_selection() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    show_left_column(&mut app);

    let explorer = geometry(&app).explorer_inner;
    click(&mut app, explorer.x + 1, explorer.y + 2);
    let selected = app.explorer.nav.cursor;
    assert_eq!(selected, 1, "test setup: a non-first entry is selected");
    click_editor_origin(&mut app);

    click(&mut app, explorer.x + 1, explorer.y);

    assert_eq!(app.focus(), Pane::Explorer);
    assert_eq!(app.explorer.nav.cursor, selected);
}

#[test]
fn a_wheel_tick_over_the_explorer_scrolls_without_taking_focus() {
    let mem = seeded_vfs(60);
    let mut app = app_with(&mem);
    show_left_column(&mut app);
    click_editor_origin(&mut app);

    let cursor_before = app.explorer.nav.cursor;
    let explorer = geometry(&app).explorer_inner;
    mouse(
        &mut app,
        MouseKind::ScrollDown,
        explorer.x + 1,
        explorer.y + 1,
    );

    assert!(app.explorer.nav.top > 0, "the wheel must scroll the window");
    assert_eq!(app.explorer.nav.cursor, cursor_before);
    assert_eq!(app.focus(), Pane::Editor);
}

#[test]
fn a_double_click_on_a_tab_row_switches_document_and_lands_on_the_editor() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    let first = app.active;
    workspace::open_path(&mut app, Path::new("/root/b.md"));
    let second = app.active;
    assert_ne!(first, second, "test setup: b.md opens as a second document");
    show_left_column(&mut app);

    let tabs = geometry(&app).tabs_inner;
    assert!(tabs.height >= 2, "test setup: both tab rows are painted");
    assert_eq!(app.tabs.nav.top, 0, "test setup: unscrolled tab list");
    click(&mut app, tabs.x + 1, tabs.y);
    click(&mut app, tabs.x + 1, tabs.y);

    assert_eq!(app.active, first);
    assert_eq!(app.focus(), Pane::Editor);
}

#[test]
fn a_single_click_on_a_tab_row_selects_it_without_switching_document() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    let first = app.active;
    workspace::open_path(&mut app, Path::new("/root/b.md"));
    let second = app.active;
    show_left_column(&mut app);

    let tabs = geometry(&app).tabs_inner;
    click(&mut app, tabs.x + 1, tabs.y);

    assert_eq!(app.focus(), Pane::Tabs);
    assert_eq!(app.tabs.nav.cursor, 0);
    assert_eq!(app.active, second);
    assert_ne!(app.active, first);
}

#[test]
fn a_double_click_on_an_explorer_file_row_opens_it() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    show_left_column(&mut app);
    assert_eq!(app.documents.order().len(), 1, "test setup: one document");

    let explorer = geometry(&app).explorer_inner;
    click(&mut app, explorer.x + 1, explorer.y + 3);
    click(&mut app, explorer.x + 1, explorer.y + 3);

    assert_eq!(app.documents.order().len(), 2);
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/b.md"))
    );
    assert_eq!(app.focus(), Pane::Editor);
}

#[test]
fn a_click_anywhere_dismisses_the_file_finder() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    let before = app.active;
    open_file_finder(&mut app);

    let editor = geometry(&app).editor;
    click(&mut app, editor.x + 2, editor.y);

    assert!(app.filesearch().is_none());
    assert_eq!(app.focus(), Pane::Editor);
    assert_eq!(app.active, before);
}

#[test]
fn a_wheel_tick_over_the_file_finder_moves_its_own_selection() {
    let mem = seeded_vfs(60);
    let mut app = app_with(&mem);
    show_left_column(&mut app);
    assert!(
        app.explorer.entries.len() > 10,
        "test setup: the explorer has room to scroll"
    );
    open_file_finder(&mut app);
    let results = app.filesearch().expect("finder open").results.len();
    assert!(
        results > 3,
        "test setup: the finder listed {results} results"
    );
    let explorer_top = app.explorer.nav.top;

    let finder = geometry(&app).explorer_inner;
    mouse(&mut app, MouseKind::ScrollDown, finder.x + 1, finder.y + 1);

    assert!(app.filesearch().expect("finder open").nav.cursor > 0);
    assert_eq!(app.explorer.nav.top, explorer_top);
}

#[test]
fn a_wheel_tick_over_the_tabs_pane_scrolls_without_taking_focus() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    show_left_column(&mut app);
    let rows = geometry(&app).tabs_inner.height as usize;
    for i in 0..rows + 4 {
        app.open_document(Buffer::new(format!("document {i}\n")));
    }
    click_editor_origin(&mut app);
    let cursor_before = app.tabs.nav.cursor;

    let tabs = geometry(&app).tabs_inner;
    mouse(&mut app, MouseKind::ScrollDown, tabs.x + 1, tabs.y + 1);

    assert!(app.tabs.nav.top > 0, "the wheel must scroll the window");
    assert_eq!(app.tabs.nav.cursor, cursor_before);
    assert_eq!(app.focus(), Pane::Editor);
}

#[test]
fn two_clicks_on_adjacent_explorer_rows_select_without_opening() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    show_left_column(&mut app);
    assert_eq!(app.documents.order().len(), 1, "test setup: one document");

    let explorer = geometry(&app).explorer_inner;
    click(&mut app, explorer.x + 1, explorer.y + 2);
    click(&mut app, explorer.x + 1, explorer.y + 3);

    assert_eq!(app.explorer.nav.cursor, 2);
    assert_eq!(app.documents.order().len(), 1);
    assert_eq!(app.focus(), Pane::Explorer);
}

#[test]
fn a_click_on_the_root_row_between_two_row_clicks_ends_the_run() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    show_left_column(&mut app);

    let explorer = geometry(&app).explorer_inner;
    click(&mut app, explorer.x + 1, explorer.y + 3);
    click(&mut app, explorer.x + 1, explorer.y);
    click(&mut app, explorer.x + 1, explorer.y + 3);

    assert_eq!(app.documents.order().len(), 1);
}

#[test]
fn two_clicks_on_adjacent_tab_rows_select_without_switching_document() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    let first = app.active;
    workspace::open_path(&mut app, Path::new("/root/b.md"));
    let second = app.active;
    assert_ne!(first, second, "test setup: b.md opens as a second document");
    show_left_column(&mut app);

    let tabs = geometry(&app).tabs_inner;
    assert!(tabs.height >= 2, "test setup: both tab rows are painted");
    click(&mut app, tabs.x + 1, tabs.y + 1);
    click(&mut app, tabs.x + 1, tabs.y);

    assert_eq!(app.tabs.nav.cursor, 0);
    assert_eq!(app.active, second);
    assert_eq!(app.focus(), Pane::Tabs);
}

#[test]
fn a_click_on_the_column_border_never_moves_the_editor_caret() {
    let mem = seeded_vfs(4);
    let mut app = app_with(&mem);
    show_left_column(&mut app);
    let editor = geometry(&app).editor;
    click(&mut app, editor.x + 4, editor.y);
    let caret = app.active_doc().cursors.primary().position;

    let block = geometry(&app)
        .left_block
        .expect("test setup: the left column is painted");
    click(&mut app, block.x, block.y + 2);
    assert_eq!(app.active_doc().cursors.primary().position, caret);

    let divider = geometry(&app)
        .tabs_divider
        .expect("test setup: the Open divider is painted");
    click(&mut app, divider.x + 1, divider.y);
    assert_eq!(app.active_doc().cursors.primary().position, caret);
}
