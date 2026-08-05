//! WP4 tripwire: proves the WP3 invariant net has no hole, deterministically
//! and once. Unlike `tests/human_session.rs`, every test here is NOT
//! `#[ignore]`d, so `make test` runs the whole file on every build (plan
//! WP4.S1).
//!
//! Two halves:
//! - one hand-written ~35-action session that must trip NOTHING
//!   (`clean_session_trips_nothing`, WP4.S2), paired with a determinism
//!   check (`driver_is_deterministic`, WP4.S5) — proof the driver itself,
//!   and the whole checker set, agree with well-formed production
//!   behaviour;
//! - one hand-built bad `Snapshot`/pair per WP3 checker, split into the
//!   sibling `tripwire_checkers` test binary (500-line budget) —
//!   the Risk R-c pattern.
//!
//! `clean_session_trips_nothing`'s fixture is curated per plan Gotcha G1:
//! plain ASCII + CJK markdown, no lists or blockquotes, no tab, no `\r` —
//! this file is NOT `#[ignore]`d, so it runs under `cargo test --workspace`,
//! which links `rune-md` with `strict-invariants` on (known-open comrak
//! sourcepos panics, `crates/rune-md/TODO.md`, Status: open).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use rune_fuzz::action::Action;
use rune_fuzz::driver;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

/// Seed content for the positive tripwire session. Headers and paragraphs
/// only — no list, no blockquote, no tab, no `\r` (plan Gotcha G1).
const FIXTURE: &str = "# Title\n\nSome prose about café and 日本語のテスト mixed in.\n\nA second paragraph follows.\n";

fn key(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

fn shift() -> Mods {
    Mods {
        shift: true,
        ..Mods::NONE
    }
}

fn sup() -> Mods {
    Mods {
        sup: true,
        ..Mods::NONE
    }
}

fn sup_shift() -> Mods {
    Mods {
        sup: true,
        shift: true,
        ..Mods::NONE
    }
}

fn ctrl() -> Mods {
    Mods {
        ctrl: true,
        ..Mods::NONE
    }
}

/// One hand-written "normal human session": type prose, navigate, extend a
/// selection, copy, move, paste (via `ClipboardReply`, answering the
/// `CmdKind::ClipboardRead` a cmd+v keystroke arms), type more, undo x3,
/// redo x2, save + deliver, resize, type, arm a save failure, save +
/// deliver again, arm the ctrl+c quit chord and let it time out (a no-op —
/// G15 — so the session survives to keep typing).
fn tripwire_script() -> Vec<Action> {
    vec![
        // type prose
        Action::Type("annotate this: ".to_string()),
        // navigate
        Action::Key(key(KeyCode::Left, Mods::NONE)),
        Action::Key(key(KeyCode::Right, Mods::NONE)),
        Action::Key(key(KeyCode::Right, Mods::NONE)),
        Action::Key(key(KeyCode::Down, Mods::NONE)),
        Action::Key(key(KeyCode::End, Mods::NONE)),
        Action::Key(key(KeyCode::Home, Mods::NONE)),
        Action::Key(key(KeyCode::Up, Mods::NONE)),
        Action::Key(key(KeyCode::PageDown, Mods::NONE)),
        Action::Key(key(KeyCode::PageUp, Mods::NONE)),
        // shift-select
        Action::Key(key(KeyCode::Right, shift())),
        Action::Key(key(KeyCode::Right, shift())),
        Action::Key(key(KeyCode::Right, shift())),
        Action::Key(key(KeyCode::Left, shift())),
        // copy (cmd+c)
        Action::Key(key(KeyCode::Char('c'), sup())),
        // move
        Action::Key(key(KeyCode::Right, Mods::NONE)),
        // paste: cmd+v arms a ClipboardRead cmd; ClipboardReply answers it
        Action::Key(key(KeyCode::Char('v'), sup())),
        Action::ClipboardReply("pasted café 文字".to_string()),
        // type more
        Action::Type("more prose after the paste".to_string()),
        // undo x3
        Action::Key(key(KeyCode::Char('z'), sup())),
        Action::Key(key(KeyCode::Char('z'), sup())),
        Action::Key(key(KeyCode::Char('z'), sup())),
        // redo x2
        Action::Key(key(KeyCode::Char('z'), sup_shift())),
        Action::Key(key(KeyCode::Char('z'), sup_shift())),
        // cmd+s, then deliver its save
        Action::Key(key(KeyCode::Char('s'), sup())),
        Action::Deliver,
        // resize
        Action::Resize(100, 30),
        // type
        Action::Type("after the resize".to_string()),
        // arm a save failure, then save + deliver it
        Action::FailNextSave,
        Action::Key(key(KeyCode::Char('s'), sup())),
        Action::Deliver,
        // arm the ctrl+c quit chord, then let the confirm window time out
        Action::Key(key(KeyCode::Char('c'), ctrl())),
        Action::ConfirmTimeout,
        // type
        Action::Type("session continues after the timeout".to_string()),
    ]
}

/// WP4.S2 — a well-formed "normal human" session must trip nothing.
#[test]
fn clean_session_trips_nothing() {
    let result = driver::run(driver::DOC_PATH, FIXTURE, &tripwire_script());
    assert!(
        result.violation.is_none(),
        "{}",
        result
            .violation
            .as_ref()
            .map(|v| format!("{}: {}", v.id, v.message))
            .unwrap_or_default()
    );

    // Defeats a no-op driver: `violation.is_none()` alone is vacuously true
    // for a driver that never delivers anything (plan Gotcha G2's class of
    // vacuous gate). `Action::Type` expands unconditionally, one step per
    // `char`, with no gate on app state (`driver.rs`'s `for ch in
    // s.chars() { ... step_and_check(...) }`) — so the four `Type` actions
    // in `tripwire_script` alone floor the step count at the sum of their
    // literal lengths, independent of whether `Deliver`/`ConfirmTimeout`
    // happened to find nothing pending: "annotate this: ".len() (15) +
    // "more prose after the paste".len() (26) + "after the resize".len()
    // (16) + "session continues after the timeout".len() (35) = 92.
    // (Observed on this script: 121 steps — every other action also
    // contributes a step here — so 92 is a deliberately conservative
    // floor, not the true count.)
    assert!(
        result.steps >= 92,
        "expected at least 92 steps (one per Type char alone), got {}; \
         a driver that never delivers a Msg would report steps=0",
        result.steps
    );

    // Defeats a no-op driver: two no-ops trivially have equal, empty
    // content. The script types text and pastes via ClipboardReply, so the
    // buffer MUST differ from the seed; if it doesn't, that's a real
    // driver defect worth surfacing, not something to paper over here.
    assert_ne!(
        result.final_content, FIXTURE,
        "the script types and pastes text; final_content identical to the seed \
         means the driver did not actually deliver those edits"
    );
}

/// WP4.S5 — the driver is deterministic given `(content, actions)`: running
/// the same script twice must produce the same violation (none here) and
/// the same final content. This is the standing guarantee WP5's shrunk
/// scripts rely on to replay forever.
#[test]
fn driver_is_deterministic() {
    let script = tripwire_script();
    let first = driver::run(driver::DOC_PATH, FIXTURE, &script);
    let second = driver::run(driver::DOC_PATH, FIXTURE, &script);
    assert_eq!(
        first.violation, second.violation,
        "violation must be deterministic across two runs of the same script"
    );
    assert_eq!(
        first.final_content, second.final_content,
        "final content must be deterministic across two runs of the same script"
    );

    // Defeats a no-op driver: two runs of a driver that never delivers
    // anything are trivially "equal" (steps=0 == steps=0). Pinning
    // `steps > 0` as well as equality rules that out — determinism must be
    // demonstrated over real work, not over two identical no-ops.
    assert_eq!(
        first.steps, second.steps,
        "step count must be deterministic across two runs of the same script"
    );
    assert!(
        first.steps > 0,
        "expected the script to actually deliver messages, got steps=0"
    );
}

/// Regression for `TODO-fuzz-undo-total-dirty-close-discard.md`: a
/// quit-chord's dirty-close Guard, armed on a document that is NOT the
/// currently active one, discards the seeded document via its own
/// `[D]iscard` key before the end-of-session undo/redo drive runs. That
/// discard is production working exactly as designed (the Guard has
/// no "but it's not the active document" exception) — the driver's own
/// end-of-session drive must recognize the seed is gone and skip `UNDO-
/// TOTAL`/`REDO-TOTAL` for the session, rather than running the drive
/// against whatever document (Help, here) is left active. Pinned in
/// `proptest-regressions/human_session.txt` too, so `make test-fuzz`
/// replays this exact shape on every run.
#[test]
fn seed_discarded_by_dirty_close_guard_skips_undo_total() {
    let actions = vec![
        Action::Type("hello world".to_string()),
        Action::Key(key(KeyCode::F1, Mods::NONE)),
        Action::Key(key(KeyCode::Char('¡'), Mods::NONE)),
        Action::Key(key(KeyCode::Char(' '), Mods::NONE)),
        Action::Key(key(KeyCode::Char('c'), ctrl())),
        Action::StaleConfirmTimeout(4294967295),
        Action::Type("\"quoted\" 'text'".to_string()),
    ];
    let result = driver::run("/fuzz/doc.md", "", &actions);
    assert!(
        result.violation.is_none(),
        "{}",
        result
            .violation
            .as_ref()
            .map(|v| format!("{}: {}", v.id, v.message))
            .unwrap_or_default()
    );
}

/// Regression for `TODO-fuzz-save-verbatim-help-doc-stale-ack.md`: `F1`
/// makes the virtual Help document active, then the quit chord's dirty-
/// close Guard arms on the REAL seeded document (still dirty, not the
/// active one) via the close-guard's scan over unpreserved dirty documents.
/// Pressing the Guard's own
/// `s` hotkey (`banner::handle_dirty_close_key`) saves THAT document, not
/// whichever one is active — production is correct here (`Msg::SaveDone`
/// already carries the right `id`). The bug was in this very fuzz driver:
/// it used to snapshot `Snapshot::content` (the ACTIVE document, Help) as
/// "the bytes this save Cmd was constructed with" instead of the target
/// document's own bytes, so `SAVE-VERBATIM` compared disk (correctly
/// holding "hello world") against the wrong document's content and misfired
/// a false positive. Pinned in `proptest-regressions/human_session.txt`
/// too, so `make test-fuzz` replays this exact shape on every run.
#[test]
fn stale_save_ack_after_help_toggle_is_attributed_to_its_own_document() {
    let actions = vec![
        Action::Type("hello world".to_string()),
        Action::Key(key(KeyCode::F1, Mods::NONE)),
        Action::Key(key(KeyCode::Char('a'), Mods::NONE)),
        Action::Key(key(KeyCode::Char('a'), Mods::NONE)),
        Action::Key(key(KeyCode::Char('c'), ctrl())),
        Action::StaleConfirmTimeout(4294967295),
        Action::Key(key(KeyCode::Char('s'), sup())),
    ];
    let result = driver::run("/fuzz/doc.md", "", &actions);
    assert!(
        result.violation.is_none(),
        "{}",
        result
            .violation
            .as_ref()
            .map(|v| format!("{}: {}", v.id, v.message))
            .unwrap_or_default()
    );
}

/// Same-document control for the fix above: an ordinary ⌘S on the SAME
/// document that stays active the whole time (no Help toggle, no
/// Guard-modal detour, so the per-document snapshot lookup degenerates to
/// exactly one candidate) must still deliver cleanly — the id-scoped
/// lookup must not regress the ordinary, non-switching case.
#[test]
fn ordinary_same_document_save_still_clean() {
    let actions = vec![
        Action::Type("hello world".to_string()),
        Action::Key(key(KeyCode::Char('s'), sup())),
        Action::Deliver,
    ];
    let result = driver::run("/fuzz/doc.md", "", &actions);
    assert!(
        result.violation.is_none(),
        "{}",
        result
            .violation
            .as_ref()
            .map(|v| format!("{}: {}", v.id, v.message))
            .unwrap_or_default()
    );
}
