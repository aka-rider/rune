//! A specification written purely in `rune_fuzz::Session` primitives: open a
//! file, type into it, click to move the caret, open a second document, and
//! switch between the two tabs the way a real user would — proving the
//! sugar Phase 3 adds (`grid`/`row`, `switch_tab_by_index`/
//! `switch_tab_by_click`) reads as a readable end-to-end vocabulary, not
//! just individually unit-tested helpers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_core::buffer::Buffer;
use rune_core::coords::BufferOffset;
use rune_fuzz::Session;
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};

#[test]
fn open_type_click_and_switch_tabs_end_to_end() {
    let mut session = Session::open("/fuzz/doc.md", "");
    assert!(session.type_("hello world").is_none());
    assert_eq!(session.snapshot().content, "hello world");

    let editor = session.snapshot().geometry.editor;
    assert!(
        session
            .mouse(MouseInput {
                kind: MouseKind::Down(MouseButton::Left),
                column: editor.x,
                row: editor.y,
                shift: false,
                alt: false,
                ctrl: false,
            })
            .is_none()
    );
    let clicked = session.snapshot();
    assert_eq!(
        clicked.cursors.first().map(|c| c.position),
        Some(BufferOffset(0)),
        "a click on the buffer's first cell puts the caret at byte 0"
    );

    let seed = clicked.active;
    let second = session
        .app_mut()
        .open_document(Buffer::new("second document"));
    let second_index = session
        .app()
        .documents
        .order()
        .iter()
        .position(|&id| id == second)
        .expect("open_document must add the new id to the tab order");

    assert!(session.switch_tab_by_index(second_index).is_none());
    let on_second = session.snapshot();
    assert_eq!(on_second.active, second);
    assert_eq!(on_second.content, "second document");

    let seed_index = session
        .app()
        .documents
        .order()
        .iter()
        .position(|&id| id == seed)
        .expect("the seeded document stays open");

    assert!(session.switch_tab_by_click(seed_index).is_none());
    let back_on_seed = session.snapshot();
    assert_eq!(back_on_seed.active, seed);
    assert_eq!(back_on_seed.content, "hello world");

    let grid = session.grid(40, 12);
    assert_eq!(grid.len(), 12);

    session.finish();
}
