#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_edit_common;

use rune_syntax::wrap::WrapMode;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{Effects, Msg};

use tui_edit_common::{ALT, HEIGHT, SHIFT, WIDTH, press, with_carets};

const ALT_SUP: Mods = Mods {
    shift: false,
    alt: true,
    ctrl: false,
    sup: true,
};
const ALT_SHIFT: Mods = Mods {
    shift: true,
    alt: true,
    ctrl: false,
    sup: false,
};
const MODES: [WrapMode; 2] = [WrapMode::Soft, WrapMode::Off];

fn carets(app: &mut App) -> Vec<usize> {
    let doc = app.active_doc_mut();
    doc.cursors.all().iter().map(|c| c.position.get()).collect()
}

fn caret(app: &mut App) -> usize {
    app.active_doc_mut().cursors.primary().position.get()
}

fn press_plain(app: &mut App, code: KeyCode) {
    press(app, code, Mods::NONE);
}

fn click_far_right_of_row(app: &mut App, row: u16) {
    let editor = rune_tui::layout::geometry(app.frame_area(), app).editor;
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::Down(MouseButton::Left),
            column: editor.x + editor.width - 1,
            row: editor.y + row,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        &mut effects,
    );
    app.sync_view();
}

fn with_wrap(marked: &str, mode: WrapMode) -> App {
    let mut app = with_carets(marked);
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Resize(WIDTH, HEIGHT), &mut effects);
    app.active_doc_mut().viewport.wrap_mode = mode;
    app.sync_view();
    app
}

fn after_end_key(marked: &str, mode: WrapMode) -> App {
    let mut app = with_wrap(marked, mode);
    press_plain(&mut app, KeyCode::End);
    app
}

#[test]
fn end_stops_before_the_cr() {
    for mode in MODES {
        let mut app = with_wrap("|ab\r\ncd", mode);
        press_plain(&mut app, KeyCode::End);
        assert_eq!(caret(&mut app), 2, "{mode:?}");
    }
}

#[test]
fn down_after_end_lands_before_the_cr_of_a_shorter_crlf_line() {
    for mode in MODES {
        let mut app = after_end_key("|abcdef\r\nA\r\nC", mode);
        assert_eq!(caret(&mut app), 6, "{mode:?}");
        press_plain(&mut app, KeyCode::Down);
        assert_eq!(caret(&mut app), 9, "{mode:?}");
    }
}

#[test]
fn up_after_end_lands_before_the_cr_of_a_shorter_crlf_line() {
    for mode in MODES {
        let mut app = after_end_key("A\r\n|abcdef\r\nC", mode);
        press_plain(&mut app, KeyCode::Up);
        assert_eq!(caret(&mut app), 1, "{mode:?}");
    }
}

#[test]
fn down_onto_an_empty_crlf_line_lands_at_its_start() {
    for mode in MODES {
        let mut app = after_end_key("|abc\r\n\r\nC", mode);
        press_plain(&mut app, KeyCode::Down);
        assert_eq!(caret(&mut app), 5, "{mode:?}");
    }
}

#[test]
fn down_onto_multibyte_text_before_a_crlf_lands_before_the_cr() {
    for mode in MODES {
        let mut app = after_end_key("|abcdef\r\nñé\r\nC", mode);
        press_plain(&mut app, KeyCode::Down);
        assert_eq!(caret(&mut app), "abcdef\r\nñé".len(), "{mode:?}");
    }
}

#[test]
fn down_onto_the_crlf_terminated_last_rows_lands_before_the_cr() {
    for mode in MODES {
        let mut app = after_end_key("|abcdef\r\nA\r\n", mode);
        press_plain(&mut app, KeyCode::Down);
        assert_eq!(caret(&mut app), 9, "{mode:?}");
        press_plain(&mut app, KeyCode::Down);
        assert_eq!(caret(&mut app), 11, "{mode:?}");
    }
}

#[test]
fn shift_down_extends_the_selection_up_to_but_not_into_the_crlf_pair() {
    for mode in MODES {
        let mut app = after_end_key("|abcdef\r\nA\r\nC", mode);
        press(&mut app, KeyCode::Down, SHIFT);
        assert_eq!(caret(&mut app), 9, "{mode:?}");
    }
}

fn line_and_col(app: &mut App) -> (usize, usize) {
    let offset = caret(app);
    let point = app.active_doc_mut().buffer.offset_to_line_col(offset);
    (point.line, point.col)
}

#[test]
fn page_down_lands_before_the_cr_of_a_shorter_crlf_line() {
    for mode in MODES {
        let marked = format!("|abcdef\r\n{}tail", "A\r\n".repeat(60));
        let mut app = after_end_key(&marked, mode);
        press_plain(&mut app, KeyCode::PageDown);
        let (line, col) = line_and_col(&mut app);
        assert!(line > 1, "{mode:?}: the page moved to line {line}");
        assert_eq!(col, 1, "{mode:?}");
    }
}

#[test]
fn page_up_lands_before_the_cr_of_a_shorter_crlf_line() {
    for mode in MODES {
        let marked = format!("{}|abcdef", "A\r\n".repeat(60));
        let mut app = after_end_key(&marked, mode);
        press_plain(&mut app, KeyCode::PageUp);
        let (line, col) = line_and_col(&mut app);
        assert!(line < 60, "{mode:?}: the page moved to line {line}");
        assert_eq!(col, 1, "{mode:?}");
    }
}

#[test]
fn clicking_past_the_end_of_a_crlf_line_lands_before_the_cr() {
    for mode in MODES {
        let mut app = with_wrap("abcdef\r\nA\r\n|C", mode);
        click_far_right_of_row(&mut app, 1);
        assert_eq!(caret(&mut app), 9, "{mode:?}");
    }
}

#[test]
fn clicking_past_the_end_of_an_empty_crlf_line_lands_at_its_start() {
    for mode in MODES {
        let mut app = with_wrap("a\r\n\r\n|C", mode);
        click_far_right_of_row(&mut app, 1);
        assert_eq!(caret(&mut app), 3, "{mode:?}");
    }
}

#[test]
fn add_cursor_below_lands_before_the_cr_of_a_shorter_crlf_line() {
    for mode in MODES {
        let mut app = after_end_key("|abcdef\r\nA\r\nC", mode);
        press(&mut app, KeyCode::Down, ALT_SUP);
        assert_eq!(carets(&mut app), vec![6, 9], "{mode:?}");
    }
}

#[test]
fn add_cursor_above_lands_before_the_cr_of_a_shorter_crlf_line() {
    for mode in MODES {
        let mut app = after_end_key("A\r\n|abcdef\r\nC", mode);
        press(&mut app, KeyCode::Up, ALT_SUP);
        assert_eq!(carets(&mut app), vec![1, 9], "{mode:?}");
    }
}

#[test]
fn right_steps_over_the_whole_crlf_pair_and_left_steps_back_over_it() {
    for mode in MODES {
        let mut app = with_wrap("ab|\r\ncd", mode);
        press_plain(&mut app, KeyCode::Right);
        assert_eq!(caret(&mut app), 4, "{mode:?}");
        press_plain(&mut app, KeyCode::Left);
        assert_eq!(caret(&mut app), 2, "{mode:?}");
    }
}

#[test]
fn word_motions_step_over_a_crlf_pair_as_one() {
    for mode in MODES {
        let mut app = with_wrap("ab|\r\n\r\ncd", mode);
        press(&mut app, KeyCode::Right, ALT);
        assert_eq!(caret(&mut app), 8, "{mode:?}");
        press(&mut app, KeyCode::Left, ALT);
        assert_eq!(caret(&mut app), 6, "{mode:?}");
    }
}

#[test]
fn clone_line_down_keeps_the_caret_before_the_cr() {
    for mode in MODES {
        let mut app = with_wrap("ab|\r\ncd", mode);
        press(&mut app, KeyCode::Down, ALT_SHIFT);
        assert_eq!(caret(&mut app), "ab\r\nab".len(), "{mode:?}");
    }
}

#[test]
fn move_line_down_keeps_the_caret_before_the_cr() {
    for mode in MODES {
        let mut app = with_wrap("abcd|\r\nx\r\nz", mode);
        press(&mut app, KeyCode::Down, ALT);
        assert_eq!(caret(&mut app), "x\r\nabcd".len(), "{mode:?}");
    }
}

#[test]
fn a_lone_cr_is_ordinary_content_for_vertical_motion() {
    for mode in MODES {
        let mut app = after_end_key("|abcdef\nA\rB\nC", mode);
        press_plain(&mut app, KeyCode::Down);
        assert_eq!(caret(&mut app), "abcdef\nA\rB".len(), "{mode:?}");
    }
}

#[test]
fn a_lone_cr_ending_the_document_is_ordinary_content_for_vertical_motion() {
    for mode in MODES {
        let mut app = after_end_key("|abcdef\nA\r", mode);
        press_plain(&mut app, KeyCode::Down);
        assert_eq!(caret(&mut app), "abcdef\nA\r".len(), "{mode:?}");
    }
}
