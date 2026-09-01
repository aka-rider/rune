use crate::app::App;
use crate::explorer::{self, ensure_visible};
use crate::explorer_preview;
use crate::explorer_search::{self, EXPLORER_SEARCH_BINDINGS};
use crate::keymap::{Binding, KeyCode, KeyInput, KeyOutcome, KeyPattern, Mods, resolve_in};
use crate::pane::Pane;
use crate::runtime::Effects;
use crate::workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplorerCommand {
    Up,
    Down,
    Top,
    Bottom,
    Open,
    ParentDir,
    Leave,
    Trash,
}

const SUP: Mods = Mods {
    shift: false,
    alt: false,
    ctrl: false,
    sup: true,
};

pub const EXPLORER_BINDINGS: &[Binding<ExplorerCommand>] = &[
    Binding {
        key: KeyPattern::new(KeyCode::Up, Mods::NONE),
        cmd: ExplorerCommand::Up,
        help: "up",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Down, Mods::NONE),
        cmd: ExplorerCommand::Down,
        help: "down",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Home, Mods::NONE),
        cmd: ExplorerCommand::Top,
        help: "top",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::End, Mods::NONE),
        cmd: ExplorerCommand::Bottom,
        help: "bottom",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Enter, Mods::NONE),
        cmd: ExplorerCommand::Open,
        help: "open",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Backspace, Mods::NONE),
        cmd: ExplorerCommand::ParentDir,
        help: "up dir",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Escape, Mods::NONE),
        cmd: ExplorerCommand::Leave,
        help: "back to editor",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Delete, Mods::NONE),
        cmd: ExplorerCommand::Trash,
        help: "trash",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Backspace, SUP),
        cmd: ExplorerCommand::Trash,
        help: "trash",
        secondary: true,
    },
];

pub fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    if let Some(cmd) = resolve_in(EXPLORER_SEARCH_BINDINGS, key)
        && (app.explorer_find().is_some() || cmd == explorer_search::ExplorerSearchCommand::Type)
    {
        explorer_search::handle_search(app, cmd, key, effects);
        return KeyOutcome::Consumed;
    }

    let Some(cmd) = resolve_in(EXPLORER_BINDINGS, key) else {
        return KeyOutcome::Ignored;
    };
    explorer_search::clear_search(app);
    match cmd {
        ExplorerCommand::Up => move_selection(app, -1, effects),
        ExplorerCommand::Down => move_selection(app, 1, effects),
        ExplorerCommand::Top => {
            app.explorer.nav.first();
            ensure_visible(app);
            explorer_preview::after_cursor_move(app, effects);
        }
        ExplorerCommand::Bottom => {
            let len = app.explorer.entries.len();
            app.explorer.nav.last(len);
            ensure_visible(app);
            explorer_preview::after_cursor_move(app, effects);
        }
        ExplorerCommand::Open => open_selected(app, effects),
        ExplorerCommand::ParentDir => go_to_parent(app, effects),
        ExplorerCommand::Leave => leave(app, effects),
        ExplorerCommand::Trash => crate::trash::request_trash(app, effects),
    }
    KeyOutcome::Consumed
}

pub(crate) fn select_index(app: &mut App, index: usize, effects: &mut Effects) {
    let len = app.explorer.entries.len();
    app.explorer.nav.cursor = index.min(len.saturating_sub(1));
    ensure_visible(app);
    explorer_preview::after_cursor_move(app, effects);
}

fn move_selection(app: &mut App, delta: isize, effects: &mut Effects) {
    let len = app.explorer.entries.len();
    app.explorer.nav.move_by(delta, len);
    select_index(app, app.explorer.nav.cursor, effects);
}

fn dangling_report(vfs: &dyn rune_vfs::Vfs, link: &std::path::Path) -> (String, String) {
    match vfs.read_link(link) {
        Err(error) => ("<unreadable>".to_string(), error.to_string()),
        Ok(target) => {
            let cause = match vfs.stat(link) {
                Err(error) => error.to_string(),
                Ok(_) => "the target resolves now; refresh the listing".to_string(),
            };
            (target.display().to_string(), cause)
        }
    }
}

pub(crate) fn open_selected(app: &mut App, effects: &mut Effects) {
    let Some((target, is_dir, link, name)) =
        app.explorer.entries.get(app.explorer.nav.cursor).map(|e| {
            (
                e.path.clone(),
                e.kind == rune_vfs::FileKind::Dir,
                e.link,
                e.name.clone(),
            )
        })
    else {
        return;
    };
    if link == rune_vfs::Link::Broken {
        let (dangling, cause) = dangling_report(app.vfs.as_ref(), &target);
        crate::messages::error(
            app,
            format!("broken symlink: {name} -> {dangling} ({cause})"),
        );
        return;
    }
    if is_dir {
        let Some(resolved) = workspace::resolve_or_report(app, &target, "open") else {
            return;
        };
        explorer::request_dir(app, resolved.into_path_buf(), effects);
        return;
    }
    let departed = crate::navhistory::departure_origin(app);

    if explorer_preview::shown_path(app) == Some(target.as_path()) {
        if let explorer_preview::Promotion::Promoted(_) = explorer_preview::promote(app, effects) {
            app.set_focus_pane(Pane::Editor, effects);
        }
        return;
    }

    app.blur_title(effects);
    if workspace::open_path_checked(app, &target, effects).is_some() {
        app.set_focus_pane(Pane::Editor, effects);
        crate::navhistory::record_departure_if_moved(app, departed);
    }
}

fn leave(app: &mut App, effects: &mut Effects) {
    let departed = crate::navhistory::departure_origin(app);
    app.set_focus_pane(Pane::Editor, effects);
    if app.focus() != Pane::Editor {
        return;
    }
    crate::navhistory::record_departure_if_moved(app, departed);
}

fn go_to_parent(app: &mut App, effects: &mut Effects) {
    let Some(parent) = app.explorer.root.parent() else {
        return;
    };
    let parent = parent.to_path_buf();
    let Some(resolved) = workspace::resolve_or_report(app, &parent, "open") else {
        return;
    };
    explorer::request_dir(app, resolved.into_path_buf(), effects);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::{DirEntry, FileKind, Mem};

    use super::*;
    use crate::runtime::DirCause;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.active_doc_mut().viewport.set_size(80, 23);
        app
    }

    fn entries(names: &[(&str, bool)]) -> Vec<DirEntry> {
        names
            .iter()
            .map(|(name, is_dir)| DirEntry {
                name: (*name).to_string(),
                path: PathBuf::from(*name),
                kind: if *is_dir {
                    FileKind::Dir
                } else {
                    FileKind::File
                },
                link: rune_vfs::Link::No,
            })
            .collect()
    }

    #[test]
    fn resolve_failure_on_a_directory_aborts_navigation_and_posts_a_message() {
        let mem = Arc::new(Mem::new());
        mem.fail_resolve(Path::new("/sub"));
        let vfs: Arc<dyn rune_vfs::Vfs + Send + Sync> = mem.clone();
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        app.active_doc_mut().viewport.set_size(80, 23);
        explorer::handle_dir_loaded(
            &mut app,
            PathBuf::from("/"),
            entries(&[("sub", true)]),
            DirCause::Nav,
            crate::generation::Generation::ZERO,
        );
        app.explorer.nav.cursor = 0;
        let root_before = app.explorer.root.clone();
        let mut effects = Effects::default();

        open_selected(&mut app, &mut effects);

        assert_eq!(
            app.explorer.root, root_before,
            "a resolve failure must never re-root the Explorer"
        );
        assert!(
            crate::messages::newest_text(&app).is_some(),
            "a resolve failure must post a message"
        );
    }

    #[test]
    fn up_and_down_clamp_at_the_list_bounds() {
        let mut app = app();
        explorer::handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("b", false), ("c", false)]),
            DirCause::Nav,
            crate::generation::Generation::ZERO,
        );
        let mut effects = Effects::default();

        let up = KeyInput {
            code: KeyCode::Up,
            mods: Mods::NONE,
        };
        assert_eq!(handle_key(&mut app, up, &mut effects), KeyOutcome::Consumed);
        assert_eq!(app.explorer.nav.cursor, 0, "clamped at the top");

        let down = KeyInput {
            code: KeyCode::Down,
            mods: Mods::NONE,
        };
        for _ in 0..10 {
            assert_eq!(
                handle_key(&mut app, down, &mut effects),
                KeyOutcome::Consumed
            );
        }
        assert_eq!(app.explorer.nav.cursor, 3, "clamped at the bottom");
    }
}
