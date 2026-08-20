use super::*;
use crate::action::{Action, HighlightVersion};
use crate::driver::DOC_PATH;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::DirCause;
use rune_vfs::DirEntry;
use std::path::PathBuf;

/// Fails loudly on an unexpected `Err` without an infallible-unwrap call
/// (keeps this whole file free of that family of call, tests included).
fn must_decode(text: &str) -> (String, String, Vec<Action>) {
    let result = decode(text);
    assert!(result.is_ok(), "decode({text:?}) failed: {result:?}");
    result.unwrap_or_else(|_| (String::new(), String::new(), Vec::new()))
}

fn key(code: KeyCode, mods: Mods) -> Action {
    Action::Key(KeyInput { code, mods })
}

fn mods(shift: bool, alt: bool, ctrl: bool, sup: bool) -> Mods {
    Mods {
        shift,
        alt,
        ctrl,
        sup,
    }
}

#[test]
fn round_trips_every_action_variant() {
    let content = "hello\nworld";
    let actions = vec![
        key(KeyCode::Char('a'), Mods::NONE),
        key(KeyCode::Left, mods(true, false, false, false)),
        key(KeyCode::Char(' '), mods(false, false, false, true)),
        key(KeyCode::Char('😀'), mods(true, true, true, true)),
        Action::Mouse(MouseInput {
            kind: MouseKind::Down(MouseButton::Left),
            column: 10,
            row: 5,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        Action::Mouse(MouseInput {
            kind: MouseKind::Up(MouseButton::Right),
            column: 0,
            row: 0,
            shift: true,
            alt: true,
            ctrl: true,
        }),
        Action::Mouse(MouseInput {
            kind: MouseKind::Drag(MouseButton::Middle),
            column: u16::MAX,
            row: u16::MAX,
            shift: false,
            alt: true,
            ctrl: false,
        }),
        Action::Mouse(MouseInput {
            kind: MouseKind::ScrollUp,
            column: 79,
            row: 23,
            shift: true,
            alt: false,
            ctrl: false,
        }),
        Action::Mouse(MouseInput {
            kind: MouseKind::ScrollDown,
            column: 1,
            row: 2,
            shift: false,
            alt: false,
            ctrl: true,
        }),
        Action::Type("some prose".to_string()),
        Action::Paste("line1\r\nline2".to_string()),
        Action::Resize(80, 24),
        Action::ClipboardReply("clipboard text".to_string()),
        Action::ConfirmTimeout,
        Action::StaleConfirmTimeout(3),
        Action::Deliver,
        Action::FailNextSave,
        Action::DirLoaded {
            entries: vec![
                DirEntry {
                    name: "sub dir".to_string(), // a literal space in the name
                    path: PathBuf::from("sub dir"),
                    kind: rune_vfs::FileKind::Dir,
                    link: rune_vfs::Link::No,
                },
                DirEntry {
                    name: "a.md".to_string(),
                    path: PathBuf::from("a.md"),
                    kind: rune_vfs::FileKind::File,
                    link: rune_vfs::Link::No,
                },
            ],
            cause: DirCause::Nav,
            generation: 7,
        },
        Action::DirLoaded {
            entries: Vec::new(),
            cause: DirCause::Refresh,
            generation: 0,
        },
        Action::Highlight {
            version: HighlightVersion::Live,
            spans: vec![(0, 3, 5), (10, 20, 0)],
        },
        Action::Highlight {
            version: HighlightVersion::Stale,
            spans: Vec::new(),
        },
        Action::Highlight {
            version: HighlightVersion::Future,
            spans: vec![(7, 2, u16::MAX)], // deliberately inverted — never validated here
        },
        Action::DivergeDisk,
        Action::DeliverDb,
        Action::DeliverDbAll,
        Action::HighlightTree {
            version: HighlightVersion::Future,
            fixture: 200,
            base: usize::MAX,
        },
        Action::InstallDiffLeft { seed_index: 0 },
        Action::InstallDiffLeft { seed_index: 255 },
    ];

    let encoded = encode(DOC_PATH, content, &actions);
    assert_eq!(
        must_decode(&encoded),
        (DOC_PATH.to_string(), content.to_string(), actions)
    );
}

#[test]
fn a_non_default_path_round_trips_via_an_explicit_path_line() {
    let content = "fn main() {}\n";
    let actions = vec![Action::Type("x".to_string())];
    let encoded = encode("/fuzz/main.rs", content, &actions);
    assert!(
        encoded.contains("\npath /fuzz/main.rs\n"),
        "expected an explicit path line, got {encoded:?}"
    );
    assert_eq!(
        must_decode(&encoded),
        ("/fuzz/main.rs".to_string(), content.to_string(), actions)
    );
}

#[test]
fn the_default_path_is_never_written_and_absence_defaults_back_to_it() {
    let encoded = encode(DOC_PATH, "hi", &[]);
    assert!(
        !encoded.contains("\npath "),
        "the default path must not be written explicitly, got {encoded:?}"
    );
    assert_eq!(must_decode(&encoded).0, DOC_PATH);
}

#[test]
fn escapes_newline_carriage_return_tab_quote_nul_and_emoji() {
    // `Action::Paste` carries arbitrary bytes verbatim (unlike `Type`,
    // which expands one `char` per keystroke and so can never deliver a
    // control char other than `\n` — see the `type`-specific case
    // below), so it exercises the escaper's fidelity for every one of
    // these chars via a full round trip.
    let cases: &[(char, &str)] = &[
        ('\n', "\\n"),
        ('\r', "\\r"),
        ('\t', "\\t"),
        ('"', "\\\""),
        ('\0', "\\u{0}"),
        ('😀', "\\u{1f600}"),
    ];
    for &(ch, want_fragment) in cases {
        let actions = vec![Action::Paste(format!("x{ch}y"))];
        let encoded = encode(DOC_PATH, "", &actions);
        assert!(
            encoded.contains(want_fragment),
            "encoding {ch:?} should contain {want_fragment:?}, got {encoded:?}"
        );
        assert_eq!(must_decode(&encoded).2, actions);
    }
}

#[test]
fn type_round_trips_the_chars_it_can_actually_deliver() {
    // `\n` (a hardcoded `KeyCode::Enter` in `driver::run`), an ordinary
    // quote, and a non-control emoji all round-trip through `type`.
    for ch in ['\n', '"', '😀'] {
        let actions = vec![Action::Type(format!("x{ch}y"))];
        let encoded = encode(DOC_PATH, "", &actions);
        assert_eq!(must_decode(&encoded).2, actions);
    }
}

#[test]
fn decode_rejects_a_control_char_in_a_type_payload() {
    // CODE-REVIEW.md rune-fuzz finding 4: `\r`/`\t`/NUL can never reach
    // a real keystroke through `Action::Type` (`is_insertable_key_char`
    // drops every control char but `\n`) — `decode` must refuse these
    // at the codec boundary instead of letting `driver::run`'s own
    // debug_assert! abort the whole harness on a replay.
    for ch in ['\r', '\t', '\0'] {
        let text = format!("content hi\ntype x{}y\n", ch.escape_default());
        let err = decode(&text).unwrap_err();
        assert!(
            matches!(err, ScriptError::UndeliverableTypeChar { ch: got, .. } if got == ch),
            "expected UndeliverableTypeChar({ch:?}), got {err:?}"
        );
    }
}

#[test]
fn rejects_a_malformed_line_with_a_typed_error() {
    let err = decode("content hi\nbogus-keyword-here\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::UnknownKeyword { ref keyword, .. }
        if keyword == "bogus-keyword-here")
    );

    let err = decode("no content line here\n").unwrap_err();
    assert!(matches!(err, ScriptError::MalformedLine { .. }));

    let err = decode("# only a comment\n\n").unwrap_err();
    assert_eq!(err, ScriptError::MissingContentLine);

    let err = decode("content hi\nkey --\n").unwrap_err();
    assert!(matches!(err, ScriptError::MalformedLine { .. }));

    let err = decode("content hi\nkey char:a xxxx\n").unwrap_err();
    assert!(matches!(err, ScriptError::InvalidMods { .. }));

    let err = decode("content hi\nresize wide tall\n").unwrap_err();
    assert!(matches!(err, ScriptError::InvalidNumber { .. }));

    let err = decode("content hi\nhighlight bogus 0\n").unwrap_err();
    assert!(matches!(err, ScriptError::MalformedLine { .. }));

    let err = decode("content hi\nhighlight live 1\n").unwrap_err();
    assert!(matches!(err, ScriptError::MalformedLine { .. }));
}

#[test]
fn highlight_tree_round_trips_every_version() {
    for version in [
        HighlightVersion::Live,
        HighlightVersion::Stale,
        HighlightVersion::Future,
    ] {
        let actions = vec![Action::HighlightTree {
            version,
            fixture: 1,
            base: 0,
        }];
        let encoded = encode(DOC_PATH, "hi", &actions);
        assert_eq!(must_decode(&encoded).2, actions);
    }
}

#[test]
fn highlight_tree_round_trips_fixture_and_base_boundaries() {
    for fixture in [0u8, 255] {
        for base in [0usize, usize::MAX] {
            let actions = vec![Action::HighlightTree {
                version: HighlightVersion::Live,
                fixture,
                base,
            }];
            let encoded = encode(DOC_PATH, "hi", &actions);
            assert_eq!(must_decode(&encoded).2, actions);
        }
    }
}

#[test]
fn highlight_tree_round_trips_amid_multiline_highlight_actions() {
    // Proves the single-line `highlight-tree` form is never confused
    // with `highlight`'s multi-line continuation form, in either
    // direction, and is never eaten by the `"highlight "` prefix
    // dispatch (they share a prefix up to the `-`/` `).
    let actions = vec![
        Action::Highlight {
            version: HighlightVersion::Live,
            spans: vec![(0, 3, 5)],
        },
        Action::HighlightTree {
            version: HighlightVersion::Stale,
            fixture: 2,
            base: 42,
        },
        Action::Highlight {
            version: HighlightVersion::Future,
            spans: vec![(1, 2, 3), (4, 5, 6)],
        },
    ];
    let encoded = encode(DOC_PATH, "hi", &actions);
    assert_eq!(must_decode(&encoded).2, actions);
}

#[test]
fn highlight_tree_rejects_malformed_inputs_with_typed_errors() {
    let err = decode("content hi\nhighlight-tree bogus 1 0\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::MalformedLine { .. }),
        "unknown version: got {err:?}"
    );

    let err = decode("content hi\nhighlight-tree live\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::MalformedLine { .. }),
        "missing fixture field: got {err:?}"
    );

    let err = decode("content hi\nhighlight-tree live 1\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::MalformedLine { .. }),
        "missing base field: got {err:?}"
    );

    let err = decode("content hi\nhighlight-tree live abc 0\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::InvalidNumber { .. }),
        "non-numeric fixture: got {err:?}"
    );

    let err = decode("content hi\nhighlight-tree live 1 abc\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::InvalidNumber { .. }),
        "non-numeric base: got {err:?}"
    );

    let err = decode("content hi\nhighlight-tree live 256 0\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::InvalidNumber { .. }),
        "fixture above u8::MAX: got {err:?}"
    );

    let err = decode("content hi\nhighlight-tree live -1 0\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::InvalidNumber { .. }),
        "negative fixture: got {err:?}"
    );

    let err = decode("content hi\nhighlight-tree live 1 -1\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::InvalidNumber { .. }),
        "negative base: got {err:?}"
    );

    let err = decode("content hi\nhighlight-tree live 1 0 extra\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::MalformedLine { .. }),
        "trailing garbage field: got {err:?}"
    );

    let err = decode("content hi\nhighlight-tree\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::UnknownKeyword { ref keyword, .. } if keyword == "highlight-tree"),
        "bare keyword with no fields: got {err:?}"
    );
}

#[test]
fn mouse_round_trips_every_kind_and_button() {
    let buttons = [MouseButton::Left, MouseButton::Right, MouseButton::Middle];
    let mut kinds: Vec<MouseKind> = vec![MouseKind::ScrollUp, MouseKind::ScrollDown];
    for b in buttons {
        kinds.extend([MouseKind::Down(b), MouseKind::Up(b), MouseKind::Drag(b)]);
    }
    for kind in kinds {
        let actions = vec![Action::Mouse(MouseInput {
            kind,
            column: 42,
            row: 7,
            shift: false,
            alt: false,
            ctrl: false,
        })];
        let encoded = encode(DOC_PATH, "hi", &actions);
        assert_eq!(must_decode(&encoded).2, actions, "kind {kind:?}");
    }
}

#[test]
fn mouse_decodes_a_hand_written_line_exactly() {
    let (_, _, actions) = must_decode("content hi\nmouse drag:left 12 3 s-c\n");
    assert_eq!(
        actions,
        vec![Action::Mouse(MouseInput {
            kind: MouseKind::Drag(MouseButton::Left),
            column: 12,
            row: 3,
            shift: true,
            alt: false,
            ctrl: true,
        })]
    );
}

#[test]
fn mouse_rejects_malformed_inputs_with_typed_errors() {
    let err = decode("content hi\nmouse hover:left 0 0 ---\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::MalformedLine { .. }),
        "unknown kind verb: got {err:?}"
    );

    let err = decode("content hi\nmouse down:pinky 0 0 ---\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::MalformedLine { .. }),
        "unknown button: got {err:?}"
    );

    let err = decode("content hi\nmouse scroll-up 0 0\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::MalformedLine { .. }),
        "missing mods field: got {err:?}"
    );

    let err = decode("content hi\nmouse down:left wide 0 ---\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::InvalidNumber { .. }),
        "non-numeric col: got {err:?}"
    );

    let err = decode("content hi\nmouse down:left 0 65536 ---\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::InvalidNumber { .. }),
        "row above u16::MAX: got {err:?}"
    );

    let err = decode("content hi\nmouse down:left 0 0 xxx\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::InvalidMods { .. }),
        "bad mods flags: got {err:?}"
    );

    let err = decode("content hi\nmouse down:left 0 0 ---u\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::InvalidMods { .. }),
        "4-char key-style mods field: got {err:?}"
    );

    let err = decode("content hi\nmouse down:left 0 0 --- extra\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::MalformedLine { .. }),
        "trailing garbage field: got {err:?}"
    );

    let err = decode("content hi\nmouse\n").unwrap_err();
    assert!(
        matches!(err, ScriptError::UnknownKeyword { ref keyword, .. } if keyword == "mouse"),
        "bare keyword with no fields: got {err:?}"
    );
}

#[test]
fn highlight_tree_decodes_a_hand_written_line_exactly() {
    let (_, _, actions) = must_decode("content hi\nhighlight-tree stale 7 42\n");
    assert_eq!(
        actions,
        vec![Action::HighlightTree {
            version: HighlightVersion::Stale,
            fixture: 7,
            base: 42,
        }]
    );
}

#[test]
fn skips_comments_and_blank_lines() {
    let text = "# a leading comment\n\ncontent hi\n\n# a comment between actions\ntype x\n";
    let (path, content, actions) = must_decode(text);
    assert_eq!(path, DOC_PATH);
    assert_eq!(content, "hi");
    assert_eq!(actions, vec![Action::Type("x".to_string())]);
}
