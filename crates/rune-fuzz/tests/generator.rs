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

use std::collections::HashSet;
use std::mem::{Discriminant, discriminant};

use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, RngSeed, TestRunner};

use rune_fuzz::action::{Action, HighlightVersion, PaletteGenClaim};
use rune_fuzz::driver;
use rune_fuzz::generate::{self, TYPE_PALETTE};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::DirCause;
use rune_vfs::DirEntry;

/// Every `TYPE_PALETTE` entry must be deliverable through `Action::Type`:
/// `Msg::Key(Char(c))` silently drops any `char::is_control()` character
/// except `'\n'` (`is_insertable_key_char`, plan Gotcha G3).
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
/// WHOLE current line (plan Gotcha G4), so a fragment like `"  a\nb"`
/// would yield `"  a\n  a"`-shaped output instead of the naive "buffer
/// gains exactly this string" expectation.
#[test]
fn a_typed_newline_actually_creates_a_line() {
    let result = driver::run(driver::DOC_PATH, "", &[Action::Type("a\nb".to_string())]);
    assert_eq!(result.violation, None, "{:?}", result.violation);
    assert_eq!(result.final_content, "a\nb");
}

/// A pasted CRLF survives byte-verbatim — `Action::Paste` is the
/// only path that inserts control bytes with no filtering (plan Gotcha
/// G3).
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
            kind: rune_vfs::FileKind::Dir,
        },
        DirEntry {
            name: String::new(),
            path: std::path::PathBuf::new(),
            kind: rune_vfs::FileKind::File,
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

const SAMPLED_SESSIONS: u32 = 1500;

fn action_variant_name(action: &Action) -> &'static str {
    match action {
        Action::Key(_) => "Key",
        Action::Mouse(_) => "Mouse",
        Action::Type(_) => "Type",
        Action::Paste(_) => "Paste",
        Action::OpenFileSearch => "OpenFileSearch",
        Action::Resize(_, _) => "Resize",
        Action::ClipboardReply(_) => "ClipboardReply",
        Action::ConfirmTimeout => "ConfirmTimeout",
        Action::StaleConfirmTimeout(_) => "StaleConfirmTimeout",
        Action::Deliver => "Deliver",
        Action::FailNextSave => "FailNextSave",
        Action::DirLoaded { .. } => "DirLoaded",
        Action::Highlight { .. } => "Highlight",
        Action::DivergeDisk => "DivergeDisk",
        Action::DeliverDb => "DeliverDb",
        Action::DeliverDbAll => "DeliverDbAll",
        Action::HighlightTree { .. } => "HighlightTree",
        Action::AdvanceClock(_) => "AdvanceClock",
        Action::PaletteRecentsLoaded { .. } => "PaletteRecentsLoaded",
    }
}

fn every_action_variant_witness() -> [Action; 19] {
    [
        Action::Key(KeyInput {
            code: KeyCode::Char('a'),
            mods: Mods::NONE,
        }),
        Action::Mouse(MouseInput {
            kind: MouseKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            shift: false,
            alt: false,
            ctrl: false,
        }),
        Action::Type(String::new()),
        Action::Paste(String::new()),
        Action::OpenFileSearch,
        Action::Resize(1, 1),
        Action::ClipboardReply(String::new()),
        Action::ConfirmTimeout,
        Action::StaleConfirmTimeout(0),
        Action::Deliver,
        Action::FailNextSave,
        Action::DirLoaded {
            entries: Vec::new(),
            cause: DirCause::Nav,
            generation: 0,
        },
        Action::Highlight {
            version: HighlightVersion::Live,
            spans: Vec::new(),
        },
        Action::DivergeDisk,
        Action::DeliverDb,
        Action::DeliverDbAll,
        Action::HighlightTree {
            version: HighlightVersion::Live,
            fixture: 0,
            base: 0,
        },
        Action::AdvanceClock(0),
        Action::PaletteRecentsLoaded {
            generation: PaletteGenClaim::Live,
            ok: true,
            names: Vec::new(),
        },
    ]
}

const EXEMPT_ACTION_VARIANTS: &[&str] = &[];

/// Samples `arb_session()` `SAMPLED_SESSIONS` times off a fixed seed
/// (deterministic, no wall-clock), collecting the `Discriminant` of every
/// produced `Action`. `every_action_variant_witness` is exhaustive over
/// `Action`, so a new variant fails this file's build until it gains a
/// witness here; an existing variant the generator never actually reaches
/// fails this test unless it is named in `EXEMPT_ACTION_VARIANTS`.
#[test]
fn arb_session_reaches_every_action_variant() {
    let config = Config {
        rng_seed: RngSeed::Fixed(0x5255_4e45),
        ..Config::default()
    };
    let mut runner = TestRunner::new(config);

    let mut seen: HashSet<Discriminant<Action>> = HashSet::new();
    let mut total_actions = 0usize;
    for _ in 0..SAMPLED_SESSIONS {
        let tree = generate::arb_session()
            .new_tree(&mut runner)
            .unwrap_or_else(|e| panic!("arb_session generation failed: {e}"));
        let (_, _, actions) = tree.current();
        total_actions += actions.len();
        seen.extend(actions.iter().map(discriminant));
    }
    assert!(
        total_actions >= 2500,
        "expected at least 2500 sampled actions across {SAMPLED_SESSIONS} sessions, got {total_actions}"
    );

    for witness in &every_action_variant_witness() {
        let name = action_variant_name(witness);
        if EXEMPT_ACTION_VARIANTS.contains(&name) {
            continue;
        }
        assert!(
            seen.contains(&discriminant(witness)),
            "Action::{name} was never produced across {SAMPLED_SESSIONS} sampled sessions \
             ({total_actions} actions); give it generator coverage or add it to \
             EXEMPT_ACTION_VARIANTS"
        );
    }
}
