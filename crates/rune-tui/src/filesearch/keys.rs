use std::path::PathBuf;

use crate::app::App;
use crate::binding::{Binding, KeyPattern, resolve_in};
use crate::keymap::{KeyCode, KeyInput, KeyOutcome, Mods};
use crate::listnav::ListCommand;
use crate::pane::Pane;
use crate::queryline;
use crate::runtime::Effects;

use super::{after_cursor_move, cancel, close, reset_and_recompute};

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

pub type FileSearchCommand = ListCommand;

pub const FILESEARCH_BINDINGS: &[Binding<FileSearchCommand>] = &[
    Binding {
        key: KeyPattern::printable(Mods::NONE),
        cmd: FileSearchCommand::Type,
        help: "type to filter",
        secondary: false,
    },
    Binding {
        key: KeyPattern::printable(SHIFT),
        cmd: FileSearchCommand::Type,
        help: "type to filter",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Backspace, Mods::NONE),
        cmd: FileSearchCommand::Erase,
        help: "erase",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Up, Mods::NONE),
        cmd: FileSearchCommand::Up,
        help: "up",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Down, Mods::NONE),
        cmd: FileSearchCommand::Down,
        help: "down",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::PageUp, Mods::NONE),
        cmd: FileSearchCommand::PageUp,
        help: "page up",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::PageDown, Mods::NONE),
        cmd: FileSearchCommand::PageDown,
        help: "page down",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Home, Mods::NONE),
        cmd: FileSearchCommand::Top,
        help: "top",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::End, Mods::NONE),
        cmd: FileSearchCommand::Bottom,
        help: "bottom",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Enter, Mods::NONE),
        cmd: FileSearchCommand::Enter,
        help: "open",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Escape, Mods::NONE),
        cmd: FileSearchCommand::Cancel,
        help: "cancel",
        secondary: false,
    },
];

pub(crate) fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    if let Some(cmd) = resolve_in(FILESEARCH_BINDINGS, key) {
        apply(app, cmd, key, effects);
    }
    KeyOutcome::Consumed
}

fn apply(app: &mut App, cmd: FileSearchCommand, key: KeyInput, effects: &mut Effects) {
    match cmd {
        FileSearchCommand::Type => {
            if let KeyCode::Char(c) = key.code {
                if let Some(state) = app.filesearch_mut() {
                    queryline::type_char(&mut state.query, c);
                }
                reset_and_recompute(app, effects);
            }
        }
        FileSearchCommand::Erase => {
            erase(app);
            reset_and_recompute(app, effects);
        }
        FileSearchCommand::Up => nav_move(app, -1, effects),
        FileSearchCommand::Down => nav_move(app, 1, effects),
        FileSearchCommand::PageUp => nav_move(app, -page_amount(app), effects),
        FileSearchCommand::PageDown => nav_move(app, page_amount(app), effects),
        FileSearchCommand::Top => nav_edge(app, true, effects),
        FileSearchCommand::Bottom => nav_edge(app, false, effects),
        FileSearchCommand::Enter => open_selected(app, effects),
        FileSearchCommand::Cancel => cancel(app, effects),
        FileSearchCommand::Tab => {}
    }
}

pub(super) fn open_selected(app: &mut App, effects: &mut Effects) {
    let Some(path) = selected_path(app) else {
        crate::messages::info(app, "no file selected");
        return;
    };
    let departed = app.filesearch().and_then(|state| state.return_to.raw());

    if let Some(id) = app.explorer.preview
        && app.doc(id).and_then(|d| d.file_path.as_deref()) == Some(path.as_path())
    {
        close(app);
        crate::explorer_preview::promote(app, id);
        app.set_focus_pane(Pane::Editor, effects);
        crate::navhistory::record_departure_if_moved(app, departed);
        return;
    }

    if crate::workspace::open_path_checked(app, &path, effects).is_some() {
        close(app);
        app.set_focus_pane(Pane::Editor, effects);
        crate::navhistory::record_departure_if_moved(app, departed);
    }
}

fn selected_path(app: &App) -> Option<PathBuf> {
    super::selected_candidate(app).map(|c| c.path.clone())
}

fn erase(app: &mut App) {
    let Some(state) = app.filesearch_mut() else {
        return;
    };
    queryline::erase_grapheme(&mut state.query);
}

pub(super) fn nav_move(app: &mut App, delta: isize, effects: &mut Effects) {
    let height = page_amount(app).max(1) as usize;
    let Some(state) = app.filesearch_mut() else {
        return;
    };
    let len = state.results.len();
    state.nav.move_and_follow(delta, len, height);
    after_cursor_move(app, effects);
}

fn nav_edge(app: &mut App, top: bool, effects: &mut Effects) {
    let Some(state) = app.filesearch_mut() else {
        return;
    };
    let len = state.results.len();
    state.nav.jump_to_edge(len, top);
    after_cursor_move(app, effects);
}

pub(crate) fn page_amount(app: &App) -> isize {
    let area = app.frame_area();
    (crate::layout::geometry(area, app).explorer_inner.height as isize)
        .saturating_sub(1)
        .max(1)
}

pub(crate) fn paste(app: &mut App, text: &str, effects: &mut Effects) {
    if app.filesearch().is_none() {
        return;
    }
    let sanitized = queryline::sanitize_pasted_line(text);
    if sanitized.is_empty() {
        return;
    }
    if let Some(state) = app.filesearch_mut() {
        state.query.push_str(&sanitized);
    }
    reset_and_recompute(app, effects);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::filesearch::Candidate;
    use crate::focus::{self, FocusTarget};
    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, Vfs, VfsTestExt};
    use std::sync::Arc;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        app
    }

    fn char_key(c: char) -> KeyInput {
        KeyInput {
            code: KeyCode::Char(c),
            mods: Mods::NONE,
        }
    }

    fn enter_key() -> KeyInput {
        KeyInput {
            code: KeyCode::Enter,
            mods: Mods::NONE,
        }
    }

    #[test]
    fn typing_appends_to_the_query() {
        let mut app = app();
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);

        assert_eq!(
            handle_key(&mut app, char_key('a'), &mut effects),
            KeyOutcome::Consumed
        );
        assert_eq!(
            handle_key(&mut app, char_key('b'), &mut effects),
            KeyOutcome::Consumed
        );

        assert_eq!(app.filesearch().map(|s| s.query.as_str()), Some("ab"));
    }

    #[test]
    fn escape_cancels_and_restores_focus() {
        let mut app = app();
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);

        let escape = KeyInput {
            code: KeyCode::Escape,
            mods: Mods::NONE,
        };
        assert_eq!(
            handle_key(&mut app, escape, &mut effects),
            KeyOutcome::Consumed
        );

        assert!(app.filesearch().is_none());
        assert_eq!(focus::target(&app), FocusTarget::Editor);
    }

    #[test]
    fn escape_through_app_update_restores_the_previously_active_document() {
        let mut app = app();
        let second = app.open_document(Buffer::new("second"));
        crate::workspace::switch_to(&mut app, second);
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);

        crate::app::update(
            &mut app,
            crate::runtime::Msg::Key(KeyInput {
                code: KeyCode::Escape,
                mods: Mods::NONE,
            }),
            &mut effects,
        );

        assert!(app.filesearch().is_none());
        assert_eq!(app.active, second);
        assert_eq!(app.focus(), crate::pane::Pane::Editor);
    }

    #[test]
    fn enter_opens_the_selected_candidate_and_returns_to_the_editor() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(std::path::Path::new("/root/a.md"), b"hello world")
            .expect("seed file");
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch().expect("open").generation;
        crate::filesearch::handle_recents_loaded(
            &mut app,
            generation,
            Ok(vec![Candidate {
                path: PathBuf::from("/root/a.md"),
                display: "a.md".to_string(),
                in_tree: true,
                mru_rank: None,
            }]),
            &mut effects,
        );

        assert_eq!(
            handle_key(&mut app, enter_key(), &mut effects),
            KeyOutcome::Consumed
        );

        assert!(app.filesearch().is_none());
        assert_eq!(app.focus(), crate::pane::Pane::Editor);
        assert_eq!(
            app.active_doc().file_path.as_deref(),
            Some(std::path::Path::new("/root/a.md"))
        );
    }

    #[test]
    fn enter_with_nothing_selected_reports_and_stays_open() {
        let mut app = app();
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);

        assert_eq!(
            handle_key(&mut app, enter_key(), &mut effects),
            KeyOutcome::Consumed
        );

        assert!(
            app.filesearch().is_some(),
            "nothing selected must never close the finder"
        );
        assert_eq!(crate::messages::newest_text(&app), Some("no file selected"));
    }

    #[test]
    fn enter_on_a_candidate_that_fails_to_read_leaves_the_finder_open_with_a_message() {
        let mut app = app();
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch().expect("open").generation;
        crate::filesearch::handle_recents_loaded(
            &mut app,
            generation,
            Ok(vec![Candidate {
                path: PathBuf::from("/root/missing.md"),
                display: "missing.md".to_string(),
                in_tree: true,
                mru_rank: None,
            }]),
            &mut effects,
        );

        assert_eq!(
            handle_key(&mut app, enter_key(), &mut effects),
            KeyOutcome::Consumed
        );

        assert!(
            app.filesearch().is_some(),
            "a failed open must leave the finder open rather than stranding the user"
        );
        assert_eq!(app.focus(), crate::pane::Pane::Explorer);
        assert!(
            crate::messages::newest_text(&app).is_some_and(|m| m.contains("could not open")),
            "the read failure must be reported"
        );
    }
}
