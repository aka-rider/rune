//! Generator self-tests (plan WP6.S5). These pin the `[fixes B4]` fix:
//! `Action::Type` can only ever deliver `'\n'` as a control character (G3),
//! and `Action::Paste` is the only path that inserts control bytes
//! verbatim. Not `#[ignore]`d — they run under plain `cargo test`/`make
//! test`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use rune_fuzz::action::Action;
use rune_fuzz::driver;
use rune_fuzz::generate::TYPE_PALETTE;
use rune_tui::runtime::DirCause;
use rune_vfs::DirEntry;

/// Every `TYPE_PALETTE` entry must be deliverable through `Action::Type`:
/// `Msg::Key(Char(c))` silently drops any `char::is_control()` character
/// except `'\n'` (`is_insertable_key_char`, `rune-tui/src/app.rs:279-281`,
/// plan Gotcha G3).
#[test]
fn type_palette_has_no_undeliverable_control_chars() {
    for entry in TYPE_PALETTE {
        for ch in entry.chars() {
            assert!(
                ch == '\n' || !ch.is_control(),
                "TYPE_PALETTE entry {:?} contains undeliverable control char {:?}",
                entry,
                ch
            );
        }
    }
}

/// A typed `'\n'` maps to `KeyCode::Enter` and actually creates a new line.
/// The fragment has NO leading whitespace: Enter auto-indents from the
/// WHOLE current line (`commands/edit.rs:196-219`, plan Gotcha G4), so a
/// fragment like `"  a\nb"` would yield `"  a\n  a"`-shaped output instead
/// of the naive "buffer gains exactly this string" expectation.
#[test]
fn a_typed_newline_actually_creates_a_line() {
    let result = driver::run(driver::DOC_PATH, "", &[Action::Type("a\nb".to_string())]);
    assert_eq!(result.violation, None, "{:?}", result.violation);
    assert_eq!(result.final_content, "a\nb");
}

/// A pasted CRLF survives byte-verbatim (§1.4.5) — `Action::Paste` is the
/// only path that inserts control bytes with no filtering
/// (`commands/clipboard.rs:112-120`, plan Gotcha G3).
#[test]
fn a_pasted_crlf_survives_verbatim() {
    let result = driver::run(
        driver::DOC_PATH,
        "",
        &[Action::Paste("line1\r\nline2".to_string())],
    );
    assert_eq!(result.violation, None, "{:?}", result.violation);
    assert_eq!(result.final_content, "line1\r\nline2");
}

/// `Action::DirLoaded` (plan WP4.S6): driving an arbitrary `Msg::DirLoaded`
/// (garbage entries, either cause) through real `update`, interleaved with
/// ordinary typing, never panics and never touches the editor's own
/// content — `explorer::handle_dir_loaded` only ever writes `App::
/// explorer`, which this content-equality assertion proves end to end.
#[test]
fn dir_loaded_never_panics_and_never_touches_editor_content() {
    let garbage_entries = vec![
        DirEntry {
            name: "\u{0}weird\u{0}".to_string(),
            path: std::path::PathBuf::from("\u{0}weird\u{0}"),
            is_dir: true,
        },
        DirEntry {
            name: String::new(),
            path: std::path::PathBuf::new(),
            is_dir: false,
        },
    ];
    let result = driver::run(
        driver::DOC_PATH,
        "hello",
        &[
            Action::Type("abc".to_string()),
            Action::DirLoaded {
                entries: garbage_entries.clone(),
                cause: DirCause::Nav,
                // `Explorer::request_generation` starts at 0 and no
                // `ReadDir` Cmd is ever issued in this driven session, so
                // `0` is the live generation — these replies are actually
                // adopted, exercising the write path this test asserts on.
                generation: 0,
            },
            Action::DirLoaded {
                entries: garbage_entries,
                cause: DirCause::Refresh,
                generation: 0,
            },
            Action::Type("def".to_string()),
        ],
    );
    assert_eq!(result.violation, None, "{:?}", result.violation);
    assert_eq!(result.final_content, "abcdefhello");
}
