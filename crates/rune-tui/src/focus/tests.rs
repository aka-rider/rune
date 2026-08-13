#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

#[test]
fn derives_from_each_pane() {
    assert_eq!(from_pane(Pane::Explorer), FocusTarget::Explorer);
    assert_eq!(from_pane(Pane::Tabs), FocusTarget::Tabs);
    assert_eq!(from_pane(Pane::Editor), FocusTarget::Editor);
    assert_eq!(from_pane(Pane::Title), FocusTarget::Title);
}

/// `target` checks the search bar's own focus bit before falling back
/// to the chrome-level `Pane` — the "second input checked first" shape
/// its own doc promises, since `Pane` never grows a search variant to
/// match on directly.
#[test]
fn target_checks_the_search_bar_before_falling_back_to_the_pane() {
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    assert_eq!(target(&app), FocusTarget::Editor);

    crate::search::open(&mut app);
    assert_eq!(target(&app), FocusTarget::SearchField);

    crate::search::close(&mut app);
    assert_eq!(target(&app), FocusTarget::Editor);
}

#[test]
fn focusing_the_explorer_captures_the_browsing_origin() {
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    app.splits.left.show();
    app.frame_width = 80;
    app.frame_height = 24;
    let editing = app.active;
    let mut effects = Effects::default();
    assert_eq!(app.explorer.browsing_origin, None);

    app.set_focus_pane(Pane::Explorer, &mut effects);

    assert_eq!(app.focus(), Pane::Explorer);
    assert_eq!(app.explorer.browsing_origin, Some(editing));
}

/// No `LayoutMode` this resolver can produce may ever call `Explorer`
/// or `Tabs` focusable while also reporting the column not painted —
/// the precise shape of the shadow-state bug this module exists to
/// close.
#[test]
fn no_mode_makes_an_unpainted_pane_focusable() {
    let editor_only = LayoutMode::EditorOnly;
    assert!(editor_only.focusable(Pane::Explorer, false).is_none());
    assert!(editor_only.focusable(Pane::Tabs, false).is_none());
    assert!(editor_only.focusable(Pane::Editor, false).is_some());
    assert!(editor_only.focusable(Pane::Title, false).is_some());

    let split_collapsed = LayoutMode::Split {
        explorer: false,
        tabs: false,
    };
    assert!(split_collapsed.focusable(Pane::Explorer, false).is_none());
    assert!(split_collapsed.focusable(Pane::Tabs, false).is_none());
    assert!(split_collapsed.focusable(Pane::Editor, false).is_some());
}

/// `Pane::Messages` is focusable exactly when `messages_open` says so —
/// unlike every other pane, whose painted-or-not state comes entirely
/// from the `LayoutMode` itself.
#[test]
fn messages_pane_is_focusable_only_when_open() {
    let mode = LayoutMode::EditorOnly;
    assert!(mode.focusable(Pane::Messages, false).is_none());
    assert!(mode.focusable(Pane::Messages, true).is_some());
}

/// `focus_or_default` never leaves a caller with nothing to focus: an
/// unpainted target always resolves to `default_focus` instead.
#[test]
fn focus_or_default_falls_back_when_the_target_is_unpainted() {
    let mode = LayoutMode::EditorOnly;
    assert_eq!(
        mode.focus_or_default(Pane::Explorer, false).pane(),
        Pane::Editor
    );
    assert_eq!(
        mode.focus_or_default(Pane::Editor, false).pane(),
        Pane::Editor
    );
}

/// The load-bearing proof `focus_or_default`'s whole guarantee rests on:
/// for every `LayoutMode` variant — `ExplorerOnly` included — and every
/// `Pane`, the result is a pane `focusable` accepts under that SAME
/// mode. Written as a loop over
/// every variant, not a handful of examples, so it keeps holding when a
/// later work package adds another `LayoutMode` or `Pane` variant: a
/// fallback that ever named an unpainted pane (the exact defect
/// `default_focus` replaced an `unwrap_or(VisiblePane(pane))` escape
/// hatch to close) fails this test immediately, for that variant.
#[test]
fn focus_or_default_never_names_a_pane_its_own_mode_refuses() {
    let modes = [
        LayoutMode::Split {
            explorer: true,
            tabs: true,
        },
        LayoutMode::Split {
            explorer: false,
            tabs: true,
        },
        LayoutMode::Split {
            explorer: true,
            tabs: false,
        },
        LayoutMode::Split {
            explorer: false,
            tabs: false,
        },
        LayoutMode::ExplorerOnly,
        LayoutMode::EditorOnly,
    ];
    let panes = [Pane::Explorer, Pane::Tabs, Pane::Editor, Pane::Title];

    for mode in modes {
        for pane in panes {
            let target = mode.focus_or_default(pane, false);
            assert!(
                mode.focusable(target.pane(), false).is_some(),
                "{mode:?}.focus_or_default({pane:?}) produced {target:?}, \
                 which {mode:?}.focusable refuses"
            );
        }
    }
}

/// The generic `focusable().is_none()` path in `reconcile`
/// must catch a pane that closes while it still holds focus, with no
/// bespoke special case: focusing the messages pane, then closing it
/// without moving focus (mirroring an async reply landing while the
/// pane happens to be focused), must still redirect focus to the
/// Editor once `reconcile` runs.
#[test]
fn reconcile_redirects_focus_off_a_pane_that_closed_while_focused() {
    use crate::runtime::Effects;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    app.frame_width = 80;
    app.frame_height = 24;

    let mut effects = Effects::default();
    crate::messages::toggle(&mut app, &mut effects);
    assert_eq!(app.focus(), Pane::Messages);

    // Closes the pane without moving focus — `messages::collapse`
    // deliberately leaves that decision to its caller.
    crate::messages::collapse(&mut app);
    assert_eq!(app.focus(), Pane::Messages, "focus untouched by collapse");

    let mut effects2 = Effects::default();
    reconcile(&mut app, &mut effects2);
    assert_eq!(
        app.focus(),
        Pane::Editor,
        "reconcile must redirect focus off a closed pane"
    );
}
