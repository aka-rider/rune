use std::path::PathBuf;

use proptest::prelude::*;

use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::DirCause;
use rune_vfs::DirEntry;

use crate::action::HighlightVersion;

pub(super) const RESIZE_MIN_WIDTH: u16 = 1;
pub(super) const RESIZE_MIN_HEIGHT: u16 = 2;
const RESIZE_MAX_WIDTH: u16 = 200;
const RESIZE_MAX_HEIGHT: u16 = 60;

pub(super) fn arb_resize() -> impl Strategy<Value = (u16, u16)> {
    (
        RESIZE_MIN_WIDTH..=RESIZE_MAX_WIDTH,
        RESIZE_MIN_HEIGHT..=RESIZE_MAX_HEIGHT,
    )
}

const MOUSE_IN_FRAME_COLUMN: std::ops::Range<u16> = 0..80;
const MOUSE_IN_FRAME_ROW: std::ops::Range<u16> = 0..24;

pub(super) fn arb_mouse_button() -> impl Strategy<Value = MouseButton> {
    prop_oneof![
        Just(MouseButton::Left),
        Just(MouseButton::Right),
        Just(MouseButton::Middle),
    ]
}

pub(super) fn arb_mouse_kind() -> impl Strategy<Value = MouseKind> {
    prop_oneof![
        arb_mouse_button().prop_map(MouseKind::Down),
        arb_mouse_button().prop_map(MouseKind::Up),
        arb_mouse_button().prop_map(MouseKind::Drag),
        Just(MouseKind::ScrollUp),
        Just(MouseKind::ScrollDown),
    ]
}

pub(super) fn arb_mouse_cell() -> impl Strategy<Value = (u16, u16)> {
    (
        prop_oneof![
            7 => MOUSE_IN_FRAME_COLUMN,
            1 => 0..RESIZE_MAX_WIDTH,
        ],
        prop_oneof![
            7 => MOUSE_IN_FRAME_ROW,
            1 => 0..RESIZE_MAX_HEIGHT,
        ],
    )
}

pub(super) fn arb_mouse_input() -> impl Strategy<Value = MouseInput> {
    (
        arb_mouse_kind(),
        arb_mouse_cell(),
        proptest::bool::weighted(0.1),
        proptest::bool::weighted(0.1),
        proptest::bool::weighted(0.1),
    )
        .prop_map(|(kind, (column, row), shift, alt, ctrl)| MouseInput {
            kind,
            column,
            row,
            shift,
            alt,
            ctrl,
        })
}

pub(super) fn arb_any_keycode() -> impl Strategy<Value = KeyCode> {
    prop_oneof![
        any::<char>().prop_map(KeyCode::Char).boxed(),
        Just(KeyCode::Enter).boxed(),
        Just(KeyCode::Backspace).boxed(),
        Just(KeyCode::Tab).boxed(),
        Just(KeyCode::Escape).boxed(),
        Just(KeyCode::Left).boxed(),
        Just(KeyCode::Right).boxed(),
        Just(KeyCode::Up).boxed(),
        Just(KeyCode::Down).boxed(),
        Just(KeyCode::Home).boxed(),
        Just(KeyCode::End).boxed(),
        Just(KeyCode::PageUp).boxed(),
        Just(KeyCode::PageDown).boxed(),
        Just(KeyCode::Delete).boxed(),
        Just(KeyCode::F1).boxed(),
    ]
}

pub(super) fn arb_mods() -> impl Strategy<Value = Mods> {
    (any::<bool>(), any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
        |(shift, alt, ctrl, sup)| Mods {
            shift,
            alt,
            ctrl,
            sup,
        },
    )
}

pub(super) fn arb_dir_entry() -> impl Strategy<Value = DirEntry> {
    let shape = prop_oneof![
        Just((rune_vfs::FileKind::File, rune_vfs::Link::No)),
        Just((rune_vfs::FileKind::Dir, rune_vfs::Link::No)),
        Just((rune_vfs::FileKind::Other, rune_vfs::Link::No)),
        Just((rune_vfs::FileKind::File, rune_vfs::Link::To)),
        Just((rune_vfs::FileKind::Dir, rune_vfs::Link::To)),
        Just((rune_vfs::FileKind::Other, rune_vfs::Link::To)),
        Just((rune_vfs::FileKind::Other, rune_vfs::Link::Broken)),
    ];
    ("[a-zA-Z0-9_.]{0,12}", shape).prop_map(|(name, (kind, link))| DirEntry {
        path: PathBuf::from(&name),
        name,
        kind,
        link,
    })
}

pub(super) fn arb_dir_cause() -> impl Strategy<Value = DirCause> {
    prop_oneof![Just(DirCause::Nav), Just(DirCause::Refresh)]
}

pub(super) fn arb_dir_loaded_generation() -> impl Strategy<Value = u32> {
    0u32..=4u32
}

pub(super) fn arb_clock_advance_millis() -> impl Strategy<Value = u64> {
    prop_oneof![
        Just(0u64),
        Just(100u64),
        Just(499u64),
        Just(500u64),
        Just(501u64),
        Just(1000u64),
    ]
}

pub(super) fn arb_highlight_version() -> impl Strategy<Value = HighlightVersion> {
    prop_oneof![
        Just(HighlightVersion::Live),
        Just(HighlightVersion::Stale),
        Just(HighlightVersion::Future),
    ]
}

pub(super) const IN_BOUNDS_START: std::ops::Range<usize> = 0..30;

pub(super) const FAR_OUT_OF_BOUNDS_START: std::ops::Range<usize> = 900..2000;

pub(super) fn arb_highlight_span() -> impl Strategy<Value = (usize, usize, u16)> {
    const IN_BOUNDS_LEN: std::ops::Range<usize> = 1..15;

    const FAR_OUT_OF_BOUNDS_LEN: std::ops::Range<usize> = 1..200;

    const INVERTED_GAP: std::ops::Range<usize> = 1..30;
    const INVERTED_END: std::ops::Range<usize> = 0..30;

    const MID_CHAR_START: std::ops::Range<usize> = 0..24;

    const SCOPE_ID: std::ops::Range<u16> = 0..30;

    prop_oneof![
        (IN_BOUNDS_START, IN_BOUNDS_LEN, SCOPE_ID).prop_map(|(start, len, scope)| (
            start,
            start + len,
            scope
        )),
        (FAR_OUT_OF_BOUNDS_START, FAR_OUT_OF_BOUNDS_LEN, SCOPE_ID)
            .prop_map(|(start, len, scope)| (start, start + len, scope)),
        (INVERTED_GAP, INVERTED_END, SCOPE_ID).prop_map(|(gap, end, scope)| (
            end + gap,
            end,
            scope
        )),
        (MID_CHAR_START, SCOPE_ID).prop_map(|(start, scope)| (start, start + 1, scope)),
    ]
}
