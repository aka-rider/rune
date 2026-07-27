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
    let result = driver::run("", &[Action::Type("a\nb".to_string())]);
    assert_eq!(result.violation, None, "{:?}", result.violation);
    assert_eq!(result.final_content, "a\nb");
}

/// A pasted CRLF survives byte-verbatim (§1.4.5) — `Action::Paste` is the
/// only path that inserts control bytes with no filtering
/// (`commands/clipboard.rs:112-120`, plan Gotcha G3).
#[test]
fn a_pasted_crlf_survives_verbatim() {
    let result = driver::run("", &[Action::Paste("line1\r\nline2".to_string())]);
    assert_eq!(result.violation, None, "{:?}", result.violation);
    assert_eq!(result.final_content, "line1\r\nline2");
}
