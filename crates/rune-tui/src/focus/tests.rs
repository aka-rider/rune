#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

#[test]
fn derives_from_each_pane() {
    assert_eq!(from_pane(Pane::Explorer), FocusTarget::Explorer);
    assert_eq!(from_pane(Pane::Tabs), FocusTarget::Tabs);
    assert_eq!(from_pane(Pane::Editor), FocusTarget::Editor);
    assert_eq!(from_pane(Pane::Title), FocusTarget::Title);
}

#[test]
fn target_checks_the_search_bar_before_falling_back_to_the_pane() {
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    assert_eq!(target(&app), FocusTarget::Editor);

    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
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
    app.frame = Some(crate::app::FrameSize::new(80, 24));
    let editing = app.active;
    let mut effects = Effects::default();
    assert_eq!(
        app.explorer.browsing_origin,
        crate::returnto::ReturnTo::none()
    );

    app.set_focus_pane(Pane::Explorer, &mut effects);

    assert_eq!(app.focus(), Pane::Explorer);
    assert_eq!(
        app.explorer.browsing_origin,
        crate::returnto::ReturnTo::to(editing)
    );
}

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

#[test]
fn messages_pane_is_focusable_only_when_open() {
    let mode = LayoutMode::EditorOnly;
    assert!(mode.focusable(Pane::Messages, false).is_none());
    assert!(mode.focusable(Pane::Messages, true).is_some());
}

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

#[test]
fn reconcile_redirects_focus_off_a_pane_that_closed_while_focused() {
    use crate::runtime::Effects;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
    app.frame = Some(crate::app::FrameSize::new(80, 24));

    let mut effects = Effects::default();
    crate::messages::toggle(&mut app, &mut effects);
    assert_eq!(app.focus(), Pane::Messages);

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
