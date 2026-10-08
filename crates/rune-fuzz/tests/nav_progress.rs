//! `NAV-PROGRESS` driven through a real session: plain Down/Up always make
//! progress, including from the last row of a GFM table whose rows are
//! wider than the terminal.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rune_fuzz::Session;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

const RIGHT: KeyInput = KeyInput {
    code: KeyCode::Right,
    mods: Mods::NONE,
};
const DOWN: KeyInput = KeyInput {
    code: KeyCode::Down,
    mods: Mods::NONE,
};

const LAST_ROW: &str =
    "| Beta | another quite long description cell to force wrapping in narrow panes | 22 |";
const WIDE_TABLE: &str = "# Wide\n\n\
    | Name | Description | Notes |\n\
    | :--- | :--- | ---: |\n\
    | Alpha | a rather long description that exceeds the usual width of a terminal | 1 |\n\
    | Beta | another quite long description cell to force wrapping in narrow panes | 22 |\n\
    \n\
    after the table\n";

fn caret(session: &Session) -> usize {
    session.app().active_doc().cursors.primary().position.get()
}

const TABLE_AT_END_OF_FILE: &str = "| Name | Description | Notes |\n\
    | :--- | :--- | ---: |\n\
    | Alpha | a rather long description that exceeds the usual width of a terminal | 1 |\n\
    | Beta | another quite long description cell to force wrapping in narrow panes | 22 |\n";

fn open_with_caret_at_end_of_last_row(content: &str, width: u16) -> Session {
    let mut session = Session::open("/doc.md", content);
    assert_eq!(session.resize(width, 24), None);
    let target = content.find(LAST_ROW).expect("fixture holds the last row") + LAST_ROW.len();
    while caret(&session) < target {
        assert_eq!(session.key(RIGHT), None);
    }
    assert_eq!(caret(&session), target);
    session
}

fn assert_down_makes_progress(content: &str, width: u16) {
    let mut session = open_with_caret_at_end_of_last_row(content, width);
    let before = caret(&session);

    let violation = session.key(DOWN).map(ToString::to_string);

    assert_eq!(violation, None);
    assert!(caret(&session) > before, "Down left the caret at {before}");
}

#[test]
fn down_from_the_end_of_a_wide_tables_last_row_makes_progress() {
    assert_down_makes_progress(WIDE_TABLE, 40);
}

#[test]
fn down_from_a_wide_table_ending_the_file_makes_progress() {
    assert_down_makes_progress(TABLE_AT_END_OF_FILE, 85);
}
