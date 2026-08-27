use crate::app::App;
use crate::keymap::GlobalCommand;
use crate::messages;
use crate::pane_bar_policy::{self, BarPolicy};
use crate::pane_global;
use crate::pane_quit::handle_quit_key;
use crate::pane_refusal::registry_refusal;
use crate::runtime::Effects;
use crate::save;

pub(crate) fn handle_global_command(app: &mut App, cmd: GlobalCommand, effects: &mut Effects) {
    let left_painted_before = matches!(
        app.layout_mode(),
        crate::focus::LayoutMode::Split { .. } | crate::focus::LayoutMode::ExplorerOnly
    );

    // Never make this an early return on refusal: a refused commit leaves
    // focus on the title, but every arm below must stay reachable regardless
    // — quit, save, and close would otherwise be unreachable while an
    // unusable name is typed.
    app.blur_title(effects);

    match pane_bar_policy::bar_policy(cmd) {
        BarPolicy::CloseBars => app.close_all_overlays(effects),
        BarPolicy::ToggleSearch => close_filesearch(app, effects),
        BarPolicy::LeaveOpen => {}
    }

    match cmd {
        GlobalCommand::ToggleLeft => pane_global::toggle_left(app, left_painted_before, effects),
        GlobalCommand::FocusTitle => {
            run_if_available(app, cmd, effects, |app, _| app.focus_title())
        }
        GlobalCommand::FocusTabs => pane_global::focus_tabs(app, effects),
        GlobalCommand::Save => run_if_available(app, cmd, effects, |app, effects| {
            let _ = save::trigger_save(
                app,
                app.active,
                save::SaveMode::Normal,
                save::SaveOrigin::Interactive,
                effects,
            );
        }),
        GlobalCommand::Help => pane_global::help(app, effects),
        GlobalCommand::QuitChord(key) => handle_quit_key(app, key, effects),
        GlobalCommand::CloseFile => run_if_available(app, cmd, effects, |app, effects| {
            crate::workspace::request_close(app, app.active, effects);
        }),
        GlobalCommand::NewDocument => pane_global::new_document(app, effects),
        GlobalCommand::TabSwitch(idx) => pane_global::tab_switch(app, idx, effects),
        GlobalCommand::ToggleReadOnly => {
            run_if_available(app, cmd, effects, |app, _| {
                crate::commands::reading::toggle(app)
            });
        }
        GlobalCommand::Merge => run_if_available(app, cmd, effects, crate::merge::toggle),
        GlobalCommand::ToggleMessages => messages::toggle(app, effects),
        GlobalCommand::Trash => pane_global::trash(app, effects),
        GlobalCommand::TogglePin => run_if_available(app, cmd, effects, |app, _| {
            crate::opentabs::limit::toggle_pin(app, app.active);
        }),
        GlobalCommand::ToggleSearch => pane_global::toggle_search(app, effects),
        GlobalCommand::SearchNext => search_step(app, true),
        GlobalCommand::SearchPrev => search_step(app, false),
        GlobalCommand::ToggleFileSearch => pane_global::toggle_file_search(app, effects),
        GlobalCommand::TogglePalette => pane_global::toggle_palette(app, effects),
        GlobalCommand::NavBack => crate::navhistory::back(app, effects),
        GlobalCommand::NavForward => crate::navhistory::forward(app, effects),
    }
}

fn run_if_available(
    app: &mut App,
    cmd: GlobalCommand,
    effects: &mut Effects,
    action: impl FnOnce(&mut App, &mut Effects),
) {
    if let Some(reason) = registry_refusal(app, crate::registry::CommandId::Global(cmd)) {
        messages::error(app, reason);
    } else {
        action(app, effects);
    }
}

// `ToggleSearch`'s own arm needs the finder-only half of
// `App::close_all_overlays` without the search-bar half: pre-closing the bar
// here would make that arm's own open/close branch always see the bar
// already closed and reopen it instead of ever closing it.
fn close_filesearch(app: &mut App, effects: &mut Effects) {
    if app.filesearch().is_some() {
        crate::filesearch::cancel(app, effects);
    }
}

fn search_step(app: &mut App, forward: bool) {
    if app.search().is_some() {
        crate::search::keys::advance(app, forward);
    } else if !crate::search::keys::advance_closed(app, forward) {
        messages::info(app, "no previous search");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::pane::Pane;
    use crate::runtime::{CmdKind, Msg};
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app() -> App {
        App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
    }

    fn fx() -> Effects {
        Effects::default()
    }

    #[test]
    fn toggle_left_shows_the_column_and_focuses_the_explorer() {
        let mut app = app();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);
        assert!(app.splits.left.is_shown());
        assert_eq!(app.focus(), Pane::Explorer);
    }

    #[test]
    fn toggle_left_hides_the_column_and_returns_focus_to_the_editor() {
        let mut app = app();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);
        assert!(!app.splits.left.is_shown());
        assert_eq!(app.focus(), Pane::Editor);
    }

    #[test]
    fn toggle_left_hide_branch_matches_a_direct_reconcile_call() {
        let mut via_command = app();
        let mut effects = Effects::default();
        via_command.splits.left.show();
        via_command.set_focus_pane(Pane::Tabs, &mut effects);
        handle_global_command(&mut via_command, GlobalCommand::ToggleLeft, &mut effects);

        let mut via_reconcile = app();
        let mut effects = Effects::default();
        via_reconcile.splits.left.show();
        via_reconcile.set_focus_pane(Pane::Tabs, &mut effects);
        via_reconcile.splits.left.hide();
        crate::focus::reconcile(&mut via_reconcile, &mut effects);

        assert_eq!(
            via_command.splits.left.is_shown(),
            via_reconcile.splits.left.is_shown()
        );
        assert_eq!(via_command.focus(), via_reconcile.focus());
        assert_eq!(via_command.focus(), Pane::Editor);
    }

    #[test]
    fn toggle_left_twice_is_identity() {
        let mut app = app();
        let before_shown = app.splits.left.is_shown();
        let before_focus = app.focus();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);
        assert_eq!(app.splits.left.is_shown(), before_shown);
        assert_eq!(app.focus(), before_focus);
    }

    #[test]
    fn toggle_left_on_a_too_small_frame_falls_back_to_the_editor() {
        let mut app = app();
        app.frame = Some(crate::app::FrameSize::new(5, 5));
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);
        assert_eq!(app.focus(), Pane::Editor);
    }

    #[test]
    fn new_document_mints_activates_and_focuses_the_title() {
        let mut app = app();
        let n = app.documents.len();
        let before = app.active;
        let mut effects = Effects::default();

        handle_global_command(&mut app, GlobalCommand::NewDocument, &mut effects);

        assert_eq!(app.documents.len(), n + 1);
        assert_ne!(app.active, before);
        assert_eq!(app.active_doc().display_name.as_deref(), Some("Untitled 1"));
        assert_eq!(app.focus(), Pane::Title);
    }

    #[test]
    fn every_focus_moving_global_closes_an_open_search_bar() {
        for cmd in [
            GlobalCommand::ToggleLeft,
            GlobalCommand::FocusTitle,
            GlobalCommand::FocusTabs,
            GlobalCommand::ToggleMessages,
            GlobalCommand::Merge,
        ] {
            let mut app = app();
            crate::search::open(&mut app, &mut fx());
            assert!(app.search().is_some(), "test setup: bar is open");

            let mut effects = Effects::default();
            handle_global_command(&mut app, cmd, &mut effects);

            assert!(app.search().is_none(), "{cmd:?} must close the search bar");
        }
    }

    #[test]
    fn sup_shift_f_chord_opens_filesearch_on_a_never_shown_left_column() {
        let mut app = app();
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        assert!(!app.splits.left.is_shown(), "test setup: column hidden");
        let mut effects = Effects::default();

        crate::app::update(
            &mut app,
            Msg::Key(crate::keymap::KeyInput {
                code: crate::keymap::KeyCode::Char('F'),
                mods: crate::keymap::Mods {
                    shift: false,
                    alt: false,
                    ctrl: false,
                    sup: true,
                },
            }),
            &mut effects,
        );

        assert!(app.filesearch().is_some());
        assert_eq!(app.focus(), Pane::Explorer);
    }

    #[test]
    fn sup_shift_f_chord_again_closes_and_restores_return_to() {
        let mut app = app();
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        let second = app.open_document(rune_core::buffer::Buffer::new("second"));
        crate::workspace::switch_to(&mut app, second);
        let mut effects = Effects::default();
        let chord = crate::keymap::KeyInput {
            code: crate::keymap::KeyCode::Char('F'),
            mods: crate::keymap::Mods {
                shift: false,
                alt: false,
                ctrl: false,
                sup: true,
            },
        };

        crate::app::update(&mut app, Msg::Key(chord), &mut effects);
        assert!(app.filesearch().is_some(), "test setup: finder open");

        crate::app::update(&mut app, Msg::Key(chord), &mut effects);

        assert!(app.filesearch().is_none());
        assert_eq!(app.active, second);
        assert_eq!(app.focus(), Pane::Editor);
    }

    #[test]
    fn table_driven_close_gate_never_strands_filesearch_off_explorer() {
        use crate::keymap::GLOBAL_BINDINGS;

        for binding in GLOBAL_BINDINGS {
            let mut app = app();
            app.frame = Some(crate::app::FrameSize::new(120, 34));
            let active = app.active;
            crate::commands::edit::insert_char(&mut app, active, '!');
            let mut effects = Effects::default();
            crate::filesearch::open(&mut app, &mut effects);
            assert!(
                app.filesearch().is_some(),
                "test setup for {:?}",
                binding.cmd
            );

            handle_global_command(&mut app, binding.cmd, &mut effects);

            assert!(
                app.filesearch().is_none() || app.focus() == Pane::Explorer,
                "{:?} left the finder open with focus on {:?}",
                binding.cmd,
                app.focus()
            );
        }
    }

    #[test]
    fn trash_while_filesearch_is_open_closes_it_instead_of_trashing() {
        let mut app = app();
        let second = app.open_document(rune_core::buffer::Buffer::new("second"));
        crate::workspace::switch_to(&mut app, second);
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);

        handle_global_command(&mut app, GlobalCommand::Trash, &mut effects);

        assert!(app.filesearch().is_none());
        assert_eq!(app.active, second);
        assert_eq!(app.focus(), Pane::Editor);
        assert!(
            effects.cmds.iter().all(|c| c.kind() != CmdKind::Trash),
            "no trash Cmd must be spawned while the finder was open"
        );
        assert!(
            messages::newest_text(&app).is_some(),
            "a message was posted"
        );
    }

    #[test]
    fn toggle_left_while_filesearch_is_open_on_a_never_shown_column_lands_on_the_editor() {
        let mut app = app();
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        assert!(
            !app.splits.left.is_shown(),
            "test setup: column never shown"
        );
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        assert!(app.filesearch().is_some(), "test setup: finder open");
        assert_eq!(
            app.focus(),
            Pane::Explorer,
            "test setup: finder's own override"
        );

        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);

        assert!(app.filesearch().is_none());
        assert_eq!(app.focus(), Pane::Editor);
        assert!(
            !app.splits.left.is_shown(),
            "the column must stay collapsed — its own Split was never really shown"
        );
    }

    #[test]
    fn toggle_search_refuses_to_open_during_an_active_merge() {
        let mut app = app();
        app.merge = crate::merge::MergeState::Active {
            doc: app.active,
            session: crate::merge::MergeSession {
                conflicts: Vec::new(),
                cur: 0,
                saved_display_name: None,
                theirs_obs: rune_db::ObsId::new(1).expect("nonzero"),
                install_pos: 0,
            },
        };
        let mut effects = Effects::default();

        handle_global_command(&mut app, GlobalCommand::ToggleSearch, &mut effects);

        assert!(app.search().is_none(), "the bar must not open mid-merge");
        assert_eq!(
            messages::newest_text(&app),
            Some("finish the merge first (^M)")
        );
    }
}
