//! Defect 1's own suite: Trash's chords are no longer global. `⌘⌫`/`^⌫`
//! used to sit in `GLOBAL_BINDINGS`, consulted before focus routing even
//! looks at which pane is focused (`dispatch::handle_key`) — reachable from
//! the editor, the title field, the finder, and the palette alike, and
//! liable to raise the Trash confirmation UNDERNEATH an already-open
//! overlay (a `modal-capture-is-total` violation). The product decision:
//! Trash is reachable only as (a) the command palette's "trash" row and
//! (b) an Explorer-pane-scoped `⌘⌫`/Delete, which only resolves while the
//! Explorer pane itself holds focus — see `explorer_keys::EXPLORER_BINDINGS`
//! and `registry/rows/global.rs`'s `trash::availability` gate. This file
//! pins both halves: every OTHER context now treats `⌘⌫`/`^⌫` as an
//! ordinary, unbound chord, and the palette-underneath hazard is
//! structurally gone rather than merely avoided at runtime. The refusal/
//! confirm semantics themselves live in the sibling `trash.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod explorer_common;
mod rename_common;
mod trash_common;

use std::path::Path;

use rune_tui::guard::GuardKind;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;

use trash_common::{app_with, ctrl_backspace, delete_key, open_palette, send, sup_backspace};

fn ctrl_cap_f() -> KeyInput {
    KeyInput {
        code: KeyCode::Char('F'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    }
}

/// `⌘⌫` in the Editor, with no Explorer focus involved, no longer raises
/// Trash — it isn't bound to anything editor-side either, so it is simply
/// consumed with no visible effect (Defect 1's report explicitly asks that
/// this stay a silent no-op rather than gain a new delete-to-line-start
/// command in this change).
#[test]
fn sup_backspace_in_the_editor_does_not_raise_trash() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    let content_before = session.app().active_doc().buffer.content().to_string();

    send(&mut session, sup_backspace());

    assert!(session.app().guard.is_none());
    assert_eq!(
        session.app().active_doc().buffer.content(),
        content_before,
        "the chord must do nothing visible in the editor"
    );
}

/// `⌘⌫` in the title field no longer commits-then-trashes: the hoisted
/// `blur_title` that used to fire ahead of the global `Trash` arm is gone
/// along with the binding, so the field is left exactly as it was.
#[test]
fn sup_backspace_in_the_title_field_does_not_raise_trash() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    rename_common::open_title(&mut session);
    let text_before = session.app().title.text().to_string();

    send(&mut session, sup_backspace());

    assert!(session.app().guard.is_none());
    assert_eq!(
        session.app().focus(),
        Pane::Title,
        "the chord must not blur the title"
    );
    assert_eq!(session.app().title.text(), text_before);
}

/// `⌘⌫` with the fuzzy file finder open no longer raises Trash underneath
/// it — the finder stays open and untouched, since the chord isn't bound in
/// that context either.
#[test]
fn sup_backspace_with_the_finder_open_does_not_raise_trash() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    session.key(ctrl_cap_f());
    assert!(
        session.app().filesearch().is_some(),
        "test setup: finder open"
    );

    send(&mut session, sup_backspace());

    assert!(session.app().guard.is_none());
    assert!(
        session.app().filesearch().is_some(),
        "the finder must stay open, untouched by the chord"
    );
}

/// The defect's own reproduction: `⌘⌫` while the command palette is open
/// used to raise the Trash guard UNDERNEATH the still-open palette overlay,
/// because `GLOBAL_BINDINGS` was consulted before focus routing ever looked
/// at the palette. With the global rows gone, the chord no longer resolves
/// at all while the palette holds focus — structurally, not just by
/// coincidence of `set_guard`'s own overlay-closing side effect.
#[test]
fn sup_backspace_with_the_palette_open_does_not_raise_trash_underneath_it() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    open_palette(&mut session);
    assert!(
        session.app().palette().is_some(),
        "test setup: palette open"
    );

    send(&mut session, sup_backspace());

    assert!(
        session.app().guard.is_none(),
        "no guard may ever be raised while the palette still owns focus"
    );
    assert!(
        session.app().palette().is_some(),
        "the palette itself must be left open and untouched"
    );
}

/// The forward-delete key, with Explorer focus, raises the Trash
/// confirmation for the selected entry — one of the two chords the product
/// decision kept.
#[test]
fn delete_key_with_explorer_focus_raises_the_guard_for_the_selected_entry() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    explorer_common::drive_load_explorer(&mut session);

    send(&mut session, delete_key());

    assert!(matches!(
        session.app().guard,
        Some(ref p) if matches!(p.kind, GuardKind::Trash { ref path, .. } if path == Path::new("/root/a.md"))
    ));
}

/// `^⌫` deliberately did NOT move to the Explorer pane along with `⌘⌫` and
/// Delete — only the two chords the product decision named survive, so
/// `^⌫` is now an ordinary unbound chord even with Explorer focus.
#[test]
fn ctrl_backspace_does_not_trash_even_with_explorer_focus() {
    let mem = explorer_common::seeded_vfs();
    let mut session = app_with(&mem);
    explorer_common::drive_load_explorer(&mut session);
    let cursor_before = session.app().explorer.nav.cursor;

    send(&mut session, ctrl_backspace());

    assert!(session.app().guard.is_none());
    assert_eq!(session.app().explorer.nav.cursor, cursor_before);
}
