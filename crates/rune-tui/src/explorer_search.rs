use crate::app::App;
use crate::binding::{Binding, KeyPattern};
use crate::explorer::ensure_visible;
use crate::explorer_preview;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::queryline;
use crate::runtime::Effects;

pub(crate) fn clear_search(app: &mut App) {
    app.close_explorer_find();
}

pub(crate) fn apply_search(app: &mut App) {
    let Some(query) = app.explorer_find() else {
        return;
    };
    let needle = query.to_lowercase();
    let hit = app
        .explorer
        .entries
        .iter()
        .position(|entry| entry.name.to_lowercase().starts_with(&needle));
    if let Some(idx) = hit {
        app.explorer.nav.cursor = idx;
        ensure_visible(app);
    }
}

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplorerSearchCommand {
    Type,
    Erase,
    Cancel,
}

pub const EXPLORER_SEARCH_BINDINGS: &[Binding<ExplorerSearchCommand>] = &[
    Binding {
        key: KeyPattern::new(KeyCode::Escape, Mods::NONE),
        cmd: ExplorerSearchCommand::Cancel,
        help: "cancel search",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Backspace, Mods::NONE),
        cmd: ExplorerSearchCommand::Erase,
        help: "erase search char",
        secondary: false,
    },
    Binding {
        key: KeyPattern::printable(Mods::NONE),
        cmd: ExplorerSearchCommand::Type,
        help: "search by name",
        secondary: false,
    },
    Binding {
        key: KeyPattern::printable(SHIFT),
        cmd: ExplorerSearchCommand::Type,
        help: "search by name",
        secondary: true,
    },
];

pub(crate) fn handle_search(
    app: &mut App,
    cmd: ExplorerSearchCommand,
    key: KeyInput,
    effects: &mut Effects,
) {
    match cmd {
        ExplorerSearchCommand::Type => {
            if let KeyCode::Char(c) = key.code {
                app.explorer_find_push(c);
                apply_search(app);
            }
        }
        ExplorerSearchCommand::Erase => {
            let emptied = match app.explorer_find_mut() {
                Some(query) => {
                    queryline::erase_grapheme(query);
                    query.is_empty()
                }
                None => false,
            };
            if emptied {
                clear_search(app);
                explorer_preview::after_cursor_move(app, effects);
            } else {
                apply_search(app);
            }
        }
        ExplorerSearchCommand::Cancel => {
            clear_search(app);
            explorer_preview::after_cursor_move(app, effects);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::{DirEntry, FileKind, Mem};

    use super::*;
    use crate::explorer;
    use crate::explorer_keys::handle_key;
    use crate::keymap::KeyOutcome;
    use crate::runtime::{DirCause, Effects};

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.active_doc_mut().viewport.set_size(80, 23);
        app
    }

    fn entries(names: &[&str]) -> Vec<DirEntry> {
        names
            .iter()
            .map(|name| DirEntry {
                name: (*name).to_string(),
                path: PathBuf::from(*name),
                kind: FileKind::File,
                link: rune_vfs::Link::No,
            })
            .collect()
    }

    fn loaded(app: &mut App, names: &[&str]) {
        explorer::handle_dir_loaded(
            app,
            PathBuf::from("/root"),
            entries(names),
            DirCause::Nav,
            crate::generation::Generation::ZERO,
        );
    }

    fn char_key(c: char) -> KeyInput {
        KeyInput {
            code: KeyCode::Char(c),
            mods: Mods::NONE,
        }
    }

    fn type_str(app: &mut App, effects: &mut Effects, s: &str) {
        for c in s.chars() {
            assert_eq!(handle_key(app, char_key(c), effects), KeyOutcome::Consumed);
        }
    }

    #[test]
    fn typing_jumps_to_the_first_case_insensitive_prefix_match() {
        let mut app = app();
        loaded(&mut app, &["Alpha", "README.md", "zeta"]);
        let mut effects = Effects::default();

        type_str(&mut app, &mut effects, "r");

        assert_eq!(app.explorer_find(), Some("r"));
        assert_eq!(
            app.explorer.entries[app.explorer.nav.cursor].name,
            "README.md"
        );
    }

    #[test]
    fn a_second_char_narrows_rather_than_restarting() {
        let mut app = app();
        loaded(&mut app, &["Alpha", "README.md", "readme2.txt", "zeta"]);
        let mut effects = Effects::default();

        type_str(&mut app, &mut effects, "re");

        assert_eq!(app.explorer_find(), Some("re"));
        assert_eq!(
            app.explorer.entries[app.explorer.nav.cursor].name,
            "README.md"
        );
    }

    #[test]
    fn a_non_matching_query_leaves_the_cursor_put_and_keeps_the_query() {
        let mut app = app();
        loaded(&mut app, &["Alpha", "README.md", "zeta"]);
        app.explorer.nav.cursor = 1;
        let mut effects = Effects::default();

        type_str(&mut app, &mut effects, "xx");

        assert_eq!(app.explorer_find(), Some("xx"));
        assert_eq!(
            app.explorer.nav.cursor, 1,
            "no match must not move the cursor"
        );
    }

    #[test]
    fn backspace_erases_one_grapheme_and_rematches_then_exits_at_length_one() {
        let mut app = app();
        loaded(&mut app, &["Alpha", "README.md", "zeta"]);
        let mut effects = Effects::default();

        type_str(&mut app, &mut effects, "re");
        assert_eq!(
            app.explorer.entries[app.explorer.nav.cursor].name,
            "README.md"
        );

        let backspace = KeyInput {
            code: KeyCode::Backspace,
            mods: Mods::NONE,
        };
        assert_eq!(
            handle_key(&mut app, backspace, &mut effects),
            KeyOutcome::Consumed
        );
        assert_eq!(app.explorer_find(), Some("r"));

        assert_eq!(
            handle_key(&mut app, backspace, &mut effects),
            KeyOutcome::Consumed
        );
        assert_eq!(
            app.explorer_find(),
            None,
            "erasing the last char exits search"
        );
    }

    #[test]
    fn backspace_when_not_searching_still_triggers_parent_dir() {
        let mut app = app();
        loaded(&mut app, &["Alpha"]);
        let mut effects = Effects::default();
        assert_eq!(app.explorer_find(), None);

        let backspace = KeyInput {
            code: KeyCode::Backspace,
            mods: Mods::NONE,
        };
        assert_eq!(
            handle_key(&mut app, backspace, &mut effects),
            KeyOutcome::Consumed
        );
        assert!(
            !effects.cmds.is_empty(),
            "Backspace with no search running must still reach ParentDir"
        );
    }

    #[test]
    fn esc_while_searching_clears_first_and_leaves_the_explorer_second() {
        let mut app = app();
        loaded(&mut app, &["Alpha", "README.md"]);
        let mut effects = Effects::default();

        type_str(&mut app, &mut effects, "r");
        let esc = KeyInput {
            code: KeyCode::Escape,
            mods: Mods::NONE,
        };
        assert_eq!(
            handle_key(&mut app, esc, &mut effects),
            KeyOutcome::Consumed
        );
        assert_eq!(app.explorer_find(), None);

        assert_eq!(
            handle_key(&mut app, esc, &mut effects),
            KeyOutcome::Consumed
        );
        assert_eq!(app.focus(), crate::pane::Pane::Editor);
    }

    #[test]
    fn nav_and_open_commands_clear_the_search() {
        let mut app = app();
        loaded(&mut app, &["Alpha", "README.md", "zeta"]);
        let mut effects = Effects::default();
        type_str(&mut app, &mut effects, "r");
        assert!(app.explorer_find().is_some());

        for (cmd, key) in [
            (
                "Up",
                KeyInput {
                    code: KeyCode::Up,
                    mods: Mods::NONE,
                },
            ),
            (
                "Down",
                KeyInput {
                    code: KeyCode::Down,
                    mods: Mods::NONE,
                },
            ),
            (
                "Home",
                KeyInput {
                    code: KeyCode::Home,
                    mods: Mods::NONE,
                },
            ),
            (
                "End",
                KeyInput {
                    code: KeyCode::End,
                    mods: Mods::NONE,
                },
            ),
        ] {
            type_str(&mut app, &mut effects, "r");
            assert!(app.explorer_find().is_some(), "setup for {cmd}");
            assert_eq!(
                handle_key(&mut app, key, &mut effects),
                KeyOutcome::Consumed,
                "{cmd} must be consumed"
            );
            assert_eq!(app.explorer_find(), None, "{cmd} must clear the search");
        }
    }

    #[test]
    fn ctrl_alt_sup_modified_char_is_not_captured() {
        let mut app = app();
        loaded(&mut app, &["Alpha"]);
        let mut effects = Effects::default();

        let ctrl_r = KeyInput {
            code: KeyCode::Char('r'),
            mods: Mods {
                shift: false,
                alt: false,
                ctrl: true,
                sup: false,
            },
        };
        assert_eq!(
            handle_key(&mut app, ctrl_r, &mut effects),
            KeyOutcome::Ignored
        );
        assert_eq!(app.explorer_find(), None);
    }
}
