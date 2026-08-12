//! Explorer type-to-search: "just start typing to jump to a file", by
//! design with no wall-clock inactivity reset — the buffer clears on blur
//! instead. State lives on `Explorer::search` (`explorer.rs`); this
//! module owns everything that reads or writes it: `clear_search`/
//! `apply_search` (moved here from `explorer.rs` — also over the 500-line
//! budget by the time this feature's own state landed), the keys that
//! drive it (`ExplorerSearchCommand`'s table — a hand-maintained key
//! list may not exist), and the handler `explorer_keys::handle_key`
//! consults before its own `EXPLORER_BINDINGS`.
//!
//! Split out of `explorer_keys.rs` to keep that file under the 500-
//! line budget once the search table and its own unit tests landed.

use unicode_segmentation::UnicodeSegmentation;

use crate::app::App;
use crate::binding::{Binding, KeyPattern};
use crate::explorer::ensure_visible;
use crate::explorer_preview;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::runtime::Effects;

/// Ends a type-to-search, if one is running. `pub(crate)`, not private:
/// `explorer_keys::handle_key` calls this from the sibling module the key
/// handling lives in, `explorer::handle_dir_loaded` calls it before
/// adopting a new listing, and `app::set_focus` calls it on blur — every
/// one of the clear points the design lists (leaving the Explorer, a
/// directory reload, Esc, an empty-query Backspace, or any ordinary
/// nav/open command) funnels through this one setter rather than each
/// writing `search = None` itself.
pub(crate) fn clear_search(app: &mut App) {
    app.close_explorer_find();
}

/// Re-runs the live query against `entries` and moves the cursor to the
/// first case-insensitive prefix match (plan "Explorer type-to-search",
/// S2): the list is never filtered, only `nav.cursor` moves, then the
/// existing `ensure_visible` scrolls it into view. The synthetic `..` row
/// (`explorer::with_parent_entry`) participates like any other entry.
///
/// A no-op search still stands when nothing matches: the cursor is left
/// wherever it was, NOT snapped back to the top, so a
/// query that overshot ("read" typed past "readme.md") lets Backspace
/// recover it rather than losing the user's place in the list.
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

/// The three keys type-to-search itself recognises (plan S3).
/// `explorer_keys::EXPLORER_BINDINGS` still owns every ordinary nav/open
/// chord — this table exists ONLY for the keys search handling must
/// intercept before that table ever sees them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplorerSearchCommand {
    Type,
    Erase,
    Cancel,
}

/// Esc/Backspace are listed FIRST: while a search is running they must beat
/// `EXPLORER_BINDINGS`'s own Esc/Backspace rows (Esc is unbound there;
/// Backspace means `ParentDir`) — `explorer_keys::handle_key`'s own
/// `is_some()` guard is what enforces that, checked before `EXPLORER_
/// BINDINGS` is even consulted.
///
/// Binding two `Type` rows, `Mods::NONE` and `Mods::SHIFT`, is what lets a
/// shifted capital letter (which arrives as `Char('A')` with `shift` set,
/// not a lowercase `Char('a')` with a separate shift flag) match at all —
/// `resolve_in` matches on one exact `KeyPattern`, so this can't be folded
/// into one row that ignores `mods` the way `KeyPattern::matches`'s whole-
/// `Mods` equality is deliberately never allowed to. The shift row is
/// marked `alias: true` so the footer's default hints don't advertise two
/// chords for what a user experiences as the same "just type" affordance.
pub const EXPLORER_SEARCH_BINDINGS: &[Binding<ExplorerSearchCommand>] = &[
    Binding {
        key: KeyPattern::new(KeyCode::Escape, Mods::NONE),
        cmd: ExplorerSearchCommand::Cancel,
        help: "cancel search",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Backspace, Mods::NONE),
        cmd: ExplorerSearchCommand::Erase,
        help: "erase search char",
        alias: false,
    },
    Binding {
        key: KeyPattern::printable(Mods::NONE),
        cmd: ExplorerSearchCommand::Type,
        help: "search by name",
        alias: false,
    },
    Binding {
        key: KeyPattern::printable(SHIFT),
        cmd: ExplorerSearchCommand::Type,
        help: "search by name",
        alias: true,
    },
];

/// Drives one `ExplorerSearchCommand` (called from `explorer_keys::
/// handle_key`, already past its `resolve_in`/`is_some()` gate). `key` is
/// only ever consulted by `Type` — `Erase`/`Cancel` need no more than the
/// command they already resolved to.
pub(crate) fn handle_search(
    app: &mut App,
    cmd: ExplorerSearchCommand,
    key: KeyInput,
    effects: &mut Effects,
) {
    match cmd {
        ExplorerSearchCommand::Type => {
            // `resolve_in` already proved `key.code` matched a `Printable`
            // pattern (a non-control `Char`) — any other shape is
            // unreachable here, so there is nothing to push on a mismatch.
            if let KeyCode::Char(c) = key.code {
                app.explorer_find_or_start().push(c);
                apply_search(app);
            }
        }
        ExplorerSearchCommand::Erase => {
            let emptied = match app.explorer_find_mut() {
                Some(query) => {
                    // Pop one GRAPHEME CLUSTER, not one `char`: a combining
                    // mark popped alone would desync what's on screen from
                    // what the buffer holds, the same reasoning `width.rs`/
                    // `breadcrumb.rs` already apply to on-screen text.
                    if let Some((byte_idx, _)) = query.grapheme_indices(true).next_back() {
                        query.truncate(byte_idx);
                    }
                    query.is_empty()
                }
                None => false,
            };
            if emptied {
                // Emptying the query EXITS search outright — this keystroke
                // must not also fall through to `ParentDir` the way a bare
                // Backspace would with no search running. Design: "clearing
                // the search produces exactly one" preview of wherever the
                // cursor landed while typing.
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
            })
            .collect()
    }

    fn loaded(app: &mut App, names: &[&str]) {
        explorer::handle_dir_loaded(
            app,
            PathBuf::from("/root"),
            entries(names),
            DirCause::Nav,
            0,
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

        // Neither "x" nor "xx" prefixes any entry (or the synthetic ".."
        // row), so this must never move the cursor from its setup value —
        // unlike a query starting with "z" (which would match "zeta" on
        // its very first character and move the cursor before the second,
        // non-matching character was even typed).
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
        // The regression this guards: a `ParentDir` issues a `ReadDir` Cmd.
        assert!(
            !effects.cmds.is_empty(),
            "Backspace with no search running must still reach ParentDir"
        );
    }

    /// A live search's first Escape ends the search only (`ExplorerSearch
    /// Command::Cancel`, gated on `app.explorer_find().is_some()`); the
    /// query is already clear by the time a second Escape arrives, so it
    /// falls through to `EXPLORER_BINDINGS`'s own `Leave` row instead —
    /// still consumed, this time landing focus on the Editor.
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
