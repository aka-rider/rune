//! `Pane::Explorer`-focused key handling (split out of `explorer.rs` per
//! the 500-line budget): `ExplorerCommand`'s binding table and `handle_key`,
//! called from `app::handle_key`'s stage-3 dispatch. Directory
//! loading/listing state stays in `explorer.rs`; `move_selection`/
//! `open_selected`/`go_to_parent` below reach back into it
//! (`explorer::ensure_visible`/`request_dir`) for the pieces
//! `handle_dir_loaded` also needs to share. Type-to-search (no wall clock)
//! is a sibling module, `explorer_search`, split out to keep this file
//! under the 500-line budget —
//! it owns `ExplorerSearchCommand`/`EXPLORER_SEARCH_BINDINGS`/
//! `handle_search`, and `handle_key` below consults it FIRST.

use crate::app::App;
use crate::explorer::{self, ensure_visible};
use crate::explorer_preview;
use crate::explorer_search::{self, EXPLORER_SEARCH_BINDINGS};
use crate::keymap::{Binding, KeyCode, KeyInput, KeyOutcome, KeyPattern, Mods, resolve_in};
use crate::pane::Pane;
use crate::runtime::Effects;
use crate::workspace;

/// The Explorer's own commands — resolved via `EXPLORER_
/// BINDINGS`, mirroring `keymap::GlobalCommand`'s shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplorerCommand {
    Up,
    Down,
    Top,
    Bottom,
    Open,
    ParentDir,
    Leave,
}

/// Arrow keys move one entry; Home/End jump to the ends; Enter opens the
/// selected entry (a file activates it, a directory navigates into it);
/// Backspace navigates to the parent of the CURRENT root (not the selected
/// entry).
pub const EXPLORER_BINDINGS: &[Binding<ExplorerCommand>] = &[
    Binding {
        key: KeyPattern::new(KeyCode::Up, Mods::NONE),
        cmd: ExplorerCommand::Up,
        help: "up",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Down, Mods::NONE),
        cmd: ExplorerCommand::Down,
        help: "down",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Home, Mods::NONE),
        cmd: ExplorerCommand::Top,
        help: "top",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::End, Mods::NONE),
        cmd: ExplorerCommand::Bottom,
        help: "bottom",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Enter, Mods::NONE),
        cmd: ExplorerCommand::Open,
        help: "open",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Backspace, Mods::NONE),
        cmd: ExplorerCommand::ParentDir,
        help: "up dir",
        alias: false,
    },
    // Only reached when `EXPLORER_SEARCH_BINDINGS`'s own Escape row didn't
    // already claim the key (`handle_key`'s `is_some()` gate below) — a
    // live search's first Escape ends the search; this row is what a
    // SECOND Escape (search already clear) falls through to.
    Binding {
        key: KeyPattern::new(KeyCode::Escape, Mods::NONE),
        cmd: ExplorerCommand::Leave,
        help: "back to editor",
        alias: false,
    },
];

/// Stage 3 of the four-stage key pipeline (plan Context, decision 8) when
/// `app.focus() == Pane::Explorer`. `effects` is needed (unlike the plan's
/// literal `handle_key(app, key) -> KeyOutcome` sketch) because `Open`/
/// `ParentDir` must enqueue a `ReadDir` `Cmd` — a Vfs read can never run
/// inline in `update` — the same reason `app::handle_editor_key`
/// this mirrors already threads `effects` through for `Save`/clipboard.
///
/// Type-to-search (`explorer_search::EXPLORER_SEARCH_BINDINGS`) is checked
/// FIRST, and only while a search is already running OR the just-typed key
/// is the `Type` row: this is the whole "there is no key that enters search
/// mode" story from the design — the very first printable keystroke both
/// starts the query and supplies its first character, so it must win over
/// `EXPLORER_BINDINGS` before that table ever sees the key (a plain letter
/// isn't bound there anyway, but Esc/Backspace ARE, and while a search is
/// live they must mean "edit the query", not "cancel"/"go to parent dir").
/// Once a normal `ExplorerCommand` fires, `clear_search` runs first — every
/// nav/open command exits a stale search, matching the design's "leaving
/// the Explorer / loading a new directory -> search cleared" list (blur and
/// directory-reload are the other two clear points, `app::set_focus` and
/// `explorer::handle_dir_loaded`).
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

/// Opens the currently selected entry: a file activates it through
/// `workspace::open_path`; a directory issues a `ReadDir` `Cmd` navigating
/// the Explorer into it. The target
/// path comes straight from `entry.path` — the byte-exact path `Vfs::
/// read_dir` returned — never rejoined from `entry.name`
/// onto `app.explorer.root`: `name` is lossy-decoded for display, and
/// rejoining it would let a byte the user's filename actually has silently
/// become U+FFFD in the path the app opens. The directory branch
/// resolves the candidate root through `workspace::resolve` first,
/// same as `initial_root`/`open_path` already do — a plain `join` would let
/// an unresolved (e.g. symlinked) path become the Explorer's new root,
/// unlike every other root-changing path in this module. Reports and
/// aborts the navigation on a `resolve` error, rather than opening a
/// directory listing under an unnormalized spelling the identity of which
/// is not actually known.
///
/// The file branch blurs the title BEFORE calling `open_path` (decision 8:
/// `open_path`'s own reactivation branch switches synchronously, and
/// `rename::begin` resolves its subject from the live `app.active`), then
/// lands focus on the Editor only when `open_path` actually returns an id —
/// a read failure raises the error banner instead and must not ALSO steal
/// the keyboard from a user still arrowing the Explorer list.
pub(crate) fn open_selected(app: &mut App, effects: &mut Effects) {
    let Some((target, is_dir)) = app
        .explorer
        .entries
        .get(app.explorer.nav.cursor)
        .map(|e| (e.path.clone(), e.kind == rune_vfs::FileKind::Dir))
    else {
        return;
    };
    if is_dir {
        let Some(resolved) = workspace::resolve_or_report(app, &target, "open") else {
            return;
        };
        explorer::request_dir(app, resolved, effects);
        return;
    }
    let departed = crate::navhistory::departure_origin(app);

    // The cursor's own preview already loaded this exact file: promote it
    // in place rather than re-reading it through `open_path` — same
    // document, same id, just no longer `Preview`. A preview still in
    // flight (or skipped outright — a search was live, the read hasn't
    // landed yet) falls through to the ordinary synchronous open below,
    // exactly as before this module existed.
    if let Some(id) = app.explorer.preview
        && app.doc(id).and_then(|d| d.file_path.as_deref()) == Some(target.as_path())
    {
        explorer_preview::promote(app, id);
        app.set_focus_pane(Pane::Editor, effects);
        crate::navhistory::record_departure_if_moved(app, departed);
        return;
    }

    app.blur_title(effects);
    // `open_path_checked` reports a read failure (via `open_path`) or a
    // full tab strip (via the limit gate) through the message log itself
    // before returning `None` — discarding the `Option` here drops only
    // the opened id, never an unsurfaced error.
    if workspace::open_path_checked(app, &target, effects).is_some() {
        app.set_focus_pane(Pane::Editor, effects);
        crate::navhistory::record_departure_if_moved(app, departed);
    }
}

/// Escape off the Explorer promotes the live preview, so it commits the
/// browse exactly like Enter does — but only once focus has actually
/// landed on the Editor: a frame too narrow to paint both panes
/// (`LayoutMode::ExplorerOnly`) resolves the Editor back to the Explorer,
/// leaving the preview live and the user exactly where they were.
fn leave(app: &mut App, effects: &mut Effects) {
    let departed = crate::navhistory::departure_origin(app);
    app.set_focus_pane(Pane::Editor, effects);
    if app.focus() != Pane::Editor {
        return;
    }
    crate::navhistory::record_departure_if_moved(app, departed);
}

/// Backspace navigates to the CURRENT root's own parent — a no-op at a
/// filesystem root (`Path::parent` returns `None`), never a Cmd for a
/// nonexistent target. Resolved through `workspace::resolve` before use (see
/// `open_selected`'s docs) — a plain `Path::parent` is pure path arithmetic
/// that never consults the filesystem, unlike `initial_root`'s own root
/// resolution.
fn go_to_parent(app: &mut App, effects: &mut Effects) {
    let Some(parent) = app.explorer.root.parent() else {
        return;
    };
    let parent = parent.to_path_buf();
    let Some(resolved) = workspace::resolve_or_report(app, &parent, "open") else {
        return;
    };
    explorer::request_dir(app, resolved, effects);
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
        // Entries are [.., a, b, c] now that "/root" has a parent — index 3
        // is the bottom, one past where it was before the leading ".." row.
        assert_eq!(app.explorer.nav.cursor, 3, "clamped at the bottom");
    }
}
