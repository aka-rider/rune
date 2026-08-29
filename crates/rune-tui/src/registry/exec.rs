use crate::app::App;
use crate::commands::{case, editor_exec, language};
use crate::pane;
use crate::pane_global;
use crate::runtime::Effects;

use super::{Availability, CommandId, PaletteCommand};

pub(crate) use crate::palette::args::ResolvedArg;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ExecOutcome {
    Done,
    Refused(String),
}

pub(crate) fn execute(
    app: &mut App,
    id: CommandId,
    arg: Option<ResolvedArg>,
    effects: &mut Effects,
) -> ExecOutcome {
    if let Availability::Unavailable(reason) = super::availability(app, id) {
        return ExecOutcome::Refused(reason.into_owned());
    }
    match id {
        CommandId::Global(cmd) => {
            pane::handle_global_command(app, cmd, effects);
            ExecOutcome::Done
        }
        CommandId::Editor(cmd) => {
            let _ = editor_exec::run(app, cmd, None, effects);
            ExecOutcome::Done
        }
        CommandId::Palette(PaletteCommand::Language) => {
            let Some(ResolvedArg::Language(choice)) = arg else {
                return ExecOutcome::Refused("language needs a choice".to_string());
            };
            language::set_language(app, app.active, choice, effects);
            ExecOutcome::Done
        }
        CommandId::Palette(PaletteCommand::TabByName) => {
            let Some(ResolvedArg::Tab(target)) = arg else {
                return ExecOutcome::Refused("tab needs a target".to_string());
            };
            let Some(idx) = app.documents.order().iter().position(|&t| t == target) else {
                return ExecOutcome::Refused("that tab is no longer open".to_string());
            };
            pane_global::tab_switch(app, idx, effects);
            ExecOutcome::Done
        }
        CommandId::Palette(PaletteCommand::Uppercase) => {
            case::uppercase(app, app.active);
            ExecOutcome::Done
        }
        CommandId::Palette(PaletteCommand::Lowercase) => {
            case::lowercase(app, app.active);
            ExecOutcome::Done
        }
        CommandId::Explorer(_)
        | CommandId::ExplorerSearch(_)
        | CommandId::Tabs(_)
        | CommandId::FileSearch(_)
        | CommandId::Diff(_)
        | CommandId::PaletteKey(_) => {
            ExecOutcome::Refused("not reachable from the palette yet".to_string())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;

    use crate::keymap::{Command, KeyCode, KeyInput, Mods, QuitKey};
    use crate::runtime::Cmd;

    use super::*;

    fn app_with(content: &str) -> App {
        let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
        let id = app.active;
        app.doc_mut(id)
            .expect("fixture doc must exist")
            .viewport
            .set_size(80, 23);
        app
    }

    #[test]
    fn execute_editor_save_matches_ctrl_s_keystroke() {
        let mut app_a = app_with("hello");
        let mut effects_a = Effects::default();
        let key = KeyInput {
            code: KeyCode::Char('s'),
            mods: Mods {
                sup: true,
                ..Mods::NONE
            },
        };
        crate::dispatch::handle_key(&mut app_a, key, &mut effects_a);

        let mut app_b = app_with("hello");
        let mut effects_b = Effects::default();
        let outcome = execute(
            &mut app_b,
            CommandId::Editor(Command::Save),
            None,
            &mut effects_b,
        );

        assert_eq!(outcome, ExecOutcome::Done);
        assert_eq!(
            app_a.active_doc().save_in_flight(),
            app_b.active_doc().save_in_flight()
        );
        let kinds_a: Vec<_> = effects_a.cmds.iter().map(Cmd::kind).collect();
        let kinds_b: Vec<_> = effects_b.cmds.iter().map(Cmd::kind).collect();
        assert_eq!(kinds_a, kinds_b);
    }

    #[test]
    fn execute_read_only_motion_matches_chord_path() {
        let mut app_a = app_with("hello world");
        app_a.active_doc_mut().read_only = crate::document::ReadOnly::Reading;
        let mut effects_a = Effects::default();
        let key = KeyInput {
            code: KeyCode::Down,
            mods: Mods::NONE,
        };
        crate::dispatch::handle_key(&mut app_a, key, &mut effects_a);

        let mut app_b = app_with("hello world");
        app_b.active_doc_mut().read_only = crate::document::ReadOnly::Reading;
        let mut effects_b = Effects::default();
        let outcome = execute(
            &mut app_b,
            CommandId::Editor(Command::Motion(
                crate::keymap::Motion::LineDown,
                crate::keymap::Extend::No,
            )),
            None,
            &mut effects_b,
        );

        assert_eq!(outcome, ExecOutcome::Done);
        assert_eq!(
            app_a.active_doc().viewport.scroll_row,
            app_b.active_doc().viewport.scroll_row
        );
        assert_eq!(
            app_a.active_doc().cursors.primary().position,
            app_b.active_doc().cursors.primary().position
        );
    }

    #[test]
    fn execute_quit_chord_arms_the_confirm() {
        let mut app = app_with("hello");
        let mut effects = Effects::default();
        let outcome = execute(
            &mut app,
            CommandId::Global(crate::global::GlobalCommand::QuitChord(QuitKey::CtrlC)),
            None,
            &mut effects,
        );
        assert_eq!(outcome, ExecOutcome::Done);
        assert!(!app.should_quit);
    }

    #[test]
    fn execute_pane_context_command_is_refused() {
        let mut app = app_with("hello");
        let mut effects = Effects::default();
        let outcome = execute(
            &mut app,
            CommandId::Explorer(crate::explorer_keys::ExplorerCommand::Up),
            None,
            &mut effects,
        );
        assert!(matches!(outcome, ExecOutcome::Refused(_)));
    }

    #[test]
    fn execute_uppercase_transforms_the_word_under_the_cursor() {
        let mut app = app_with("hello world");
        let id = app.active;
        let mut effects = Effects::default();
        let outcome = execute(
            &mut app,
            CommandId::Palette(PaletteCommand::Uppercase),
            None,
            &mut effects,
        );
        assert_eq!(outcome, ExecOutcome::Done);
        assert_eq!(app.doc(id).unwrap().buffer.content(), "HELLO world");
    }

    #[test]
    fn execute_an_unavailable_command_is_refused_without_running_it() {
        let mut app = app_with("hello");
        let mut effects = Effects::default();
        let outcome = execute(
            &mut app,
            CommandId::Global(crate::global::GlobalCommand::Merge),
            None,
            &mut effects,
        );
        assert!(
            matches!(outcome, ExecOutcome::Refused(_)),
            "a command the registry marks Unavailable must never report Done"
        );
    }

    #[test]
    fn execute_language_without_a_resolved_arg_is_refused() {
        let mut app = app_with("hello");
        let mut effects = Effects::default();
        let outcome = execute(
            &mut app,
            CommandId::Palette(PaletteCommand::Language),
            None,
            &mut effects,
        );
        assert!(matches!(outcome, ExecOutcome::Refused(_)));
    }
}
