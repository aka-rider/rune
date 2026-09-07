#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use ratatui::buffer::Buffer as RtBuffer;
use ratatui::style::Color;

use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pointer::ManualClock;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

mod tui_render_common;
use tui_render_common::{HEIGHT, WIDTH, render_to_test_backend};

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

const CTRL: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: true,
    sup: false,
};

fn send(app: &mut App, msg: Msg) {
    let mut effects = Effects::default();
    rune_tui::app::update(app, msg, &mut effects);
}

fn key(app: &mut App, code: KeyCode, mods: Mods) {
    send(app, Msg::Key(KeyInput { code, mods }));
    app.sync_view();
}

fn app_sized(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.clock = Arc::new(ManualClock::new());
    app.frame = Some(rune_tui::app::FrameSize::new(WIDTH, HEIGHT));
    app.sync_view();
    app
}

fn editor_origin(app: &App) -> (u16, u16) {
    let area = app.frame_area();
    let editor = rune_tui::layout::geometry(area, app).editor;
    (editor.x, editor.y)
}

fn select_bytes(app: &mut App, start: usize, len: usize) {
    for _ in 0..start {
        key(app, KeyCode::Right, Mods::NONE);
    }
    for _ in 0..len {
        key(app, KeyCode::Right, SHIFT);
    }
    let cursor = app.active_doc().cursors.primary();
    let (from, to) = cursor.selection_range();
    assert_eq!(
        (from.get(), to.get()),
        (start, start + len),
        "the key walk must land on exactly the intended selection"
    );
}

fn bg_of(buf: &RtBuffer, ox: u16, oy: u16, column: usize) -> Color {
    buf.cell((ox + u16::try_from(column).expect("column fits"), oy))
        .expect("the editor cell is on screen")
        .bg
}

fn any_cell_has_bg(buf: &RtBuffer, bg: Color) -> bool {
    (0..HEIGHT).any(|y| (0..WIDTH).any(|x| buf.cell((x, y)).is_some_and(|c| c.bg == bg)))
}

fn hint_bg(app: &App) -> Color {
    app.theme
        .chrome
        .selection_match_bg
        .bg
        .expect("the hint carries a background")
}

const FOXES: &str = "the fox saw a fox and foxes\n";

#[test]
fn another_whole_word_occurrence_of_the_selection_gets_the_hint() {
    let mut app = app_sized(FOXES);
    select_bytes(&mut app, 4, 3);

    let buf = render_to_test_backend(&app);
    let (ox, oy) = editor_origin(&app);
    let hint = hint_bg(&app);

    for column in 14..17 {
        assert_eq!(
            bg_of(&buf, ox, oy, column),
            hint,
            "column {column} of the second `fox` must carry the hint"
        );
    }
    for column in 4..7 {
        assert_eq!(
            bg_of(&buf, ox, oy, column),
            app.theme.chrome.selection_bg,
            "column {column} of the selection itself must keep the selection background"
        );
    }
    for column in 22..27 {
        assert_ne!(
            bg_of(&buf, ox, oy, column),
            hint,
            "column {column} of `foxes` must not be hinted for a whole-word selection"
        );
    }
}

#[test]
fn a_partial_word_selection_matches_inside_a_longer_word() {
    let mut app = app_sized(FOXES);
    select_bytes(&mut app, 4, 2);

    let buf = render_to_test_backend(&app);
    let (ox, oy) = editor_origin(&app);
    let hint = hint_bg(&app);

    for column in [14, 15, 22, 23] {
        assert_eq!(
            bg_of(&buf, ox, oy, column),
            hint,
            "column {column} must carry the hint for the partial selection `fo`"
        );
    }
}

#[test]
fn the_hint_is_case_sensitive() {
    let mut app = app_sized("the Fox saw a fox\n");
    select_bytes(&mut app, 4, 3);

    let buf = render_to_test_backend(&app);
    let hint = hint_bg(&app);

    assert!(
        !any_cell_has_bg(&buf, hint),
        "`Fox` must not hint the lowercase `fox`"
    );
}

#[test]
fn a_selection_spanning_a_line_break_hints_nothing() {
    let mut app = app_sized("fox\nfox\n");
    select_bytes(&mut app, 0, 5);

    let buf = render_to_test_backend(&app);
    assert!(
        !any_cell_has_bg(&buf, hint_bg(&app)),
        "a multi-line selection must produce no hints"
    );
}

#[test]
fn a_collapsed_cursor_hints_nothing() {
    let mut app = app_sized(FOXES);
    for _ in 0..6 {
        key(&mut app, KeyCode::Right, Mods::NONE);
    }
    assert!(!app.active_doc().cursors.primary().has_selection());

    let buf = render_to_test_backend(&app);
    assert!(
        !any_cell_has_bg(&buf, hint_bg(&app)),
        "a caret without a selection must produce no hints"
    );
}

#[test]
fn an_unfocused_document_hints_nothing() {
    let mut app = app_sized(FOXES);
    select_bytes(&mut app, 4, 3);
    key(&mut app, KeyCode::Char('b'), CTRL);
    assert!(!app.active_doc().focused);

    let buf = render_to_test_backend(&app);
    assert!(
        !any_cell_has_bg(&buf, hint_bg(&app)),
        "an unfocused document must produce no hints"
    );
}

// An open find panel blurs the document (`sync_view` clears `focused` while
// one is open), so the two backgrounds can never contend for the same cell:
// the panel's own match painting is what survives. The selection seeds the
// query, so the selected occurrence is the current match.
#[test]
fn an_in_file_search_keeps_its_own_match_background() {
    let mut app = app_sized(FOXES);
    select_bytes(&mut app, 4, 3);
    key(&mut app, KeyCode::Char('f'), CTRL);
    assert_eq!(
        app.find_draft(),
        Some("fox"),
        "the selection seeds the query"
    );
    assert!(
        app.active_doc().cursors.primary().has_selection(),
        "the selection must outlive opening the find panel"
    );

    let buf = render_to_test_backend(&app);
    let (ox, oy) = editor_origin(&app);
    let search_bg = app
        .theme
        .chrome
        .search_match_bg
        .bg
        .expect("the search match carries a background");
    let current_bg = app
        .theme
        .chrome
        .search_current_bg
        .bg
        .expect("the current match carries a background");

    assert_eq!(
        bg_of(&buf, ox, oy, 4),
        current_bg,
        "the seeded occurrence is the current match"
    );
    for column in [14, 22] {
        assert_eq!(
            bg_of(&buf, ox, oy, column),
            search_bg,
            "column {column} must keep the search match background"
        );
    }
    assert!(
        !any_cell_has_bg(&buf, hint_bg(&app)),
        "a blurred document paints no selection hint under the search matches"
    );
}
