//! `resolve_in`/`KeyPattern`/`GLOBAL_BINDINGS` coverage (plan WP2.S7) — kept
//! out-of-crate per `keymap.rs`'s own note: every item exercised here is
//! already `pub`, and this keeps `keymap.rs` closer to the 500-line budget.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rune_tui::keymap::{
    GLOBAL_BINDINGS, GlobalCommand, KeyCode, KeyInput, KeyPattern, Mods, QuitKey, resolve_in,
};

fn key(code: KeyCode, mods: Mods) -> KeyInput {
    KeyInput { code, mods }
}

#[test]
fn resolve_in_matches_the_exact_modifier_set() {
    let ctrl_b = key(
        KeyCode::Char('b'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );
    assert_eq!(
        resolve_in(GLOBAL_BINDINGS, ctrl_b),
        Some(GlobalCommand::ToggleLeft)
    );
}

#[test]
fn resolve_in_rejects_an_extra_held_modifier() {
    // Same code+ctrl as `resolve_in_matches_the_exact_modifier_set`, plus
    // shift — `KeyPattern` matches the WHOLE `Mods` set, so this must NOT
    // resolve to `FocusExplorer`.
    let ctrl_shift_b = key(
        KeyCode::Char('b'),
        Mods {
            ctrl: true,
            shift: true,
            ..Mods::NONE
        },
    );
    assert_eq!(resolve_in(GLOBAL_BINDINGS, ctrl_shift_b), None);
}

#[test]
fn resolve_in_rejects_an_unbound_code() {
    let ctrl_q = key(
        KeyCode::Char('q'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );
    assert_eq!(resolve_in(GLOBAL_BINDINGS, ctrl_q), None);
}

#[test]
fn global_bindings_cover_quit_focus_save_and_help() {
    let f1 = key(KeyCode::F1, Mods::NONE);
    assert_eq!(resolve_in(GLOBAL_BINDINGS, f1), Some(GlobalCommand::Help));

    let sup_s = key(
        KeyCode::Char('s'),
        Mods {
            sup: true,
            ..Mods::NONE
        },
    );
    assert_eq!(
        resolve_in(GLOBAL_BINDINGS, sup_s),
        Some(GlobalCommand::Save)
    );

    let ctrl_s = key(
        KeyCode::Char('s'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );
    assert_eq!(
        resolve_in(GLOBAL_BINDINGS, ctrl_s),
        Some(GlobalCommand::Save)
    );

    let ctrl_d = key(
        KeyCode::Char('d'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );
    assert_eq!(
        resolve_in(GLOBAL_BINDINGS, ctrl_d),
        Some(GlobalCommand::QuitChord(QuitKey::CtrlD))
    );
}

#[test]
fn key_pattern_label_renders_the_modifier_prefix_and_uppercased_char() {
    let ctrl_b = KeyPattern::new(
        KeyCode::Char('b'),
        Mods {
            ctrl: true,
            ..Mods::NONE
        },
    );
    assert_eq!(ctrl_b.label(), "^B");

    let sup_s = KeyPattern::new(
        KeyCode::Char('s'),
        Mods {
            sup: true,
            ..Mods::NONE
        },
    );
    assert_eq!(sup_s.label(), "\u{2318}S");

    assert_eq!(KeyPattern::new(KeyCode::F1, Mods::NONE).label(), "F1");
}
