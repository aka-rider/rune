//! `Pane::Explorer`-focused key handling (split out of `explorer.rs` per
//! §1.6): `ExplorerCommand`'s binding table and `handle_key`, called from
//! `app::handle_key`'s stage-3 dispatch. Directory loading/listing state
//! stays in `explorer.rs`; `move_selection`/`open_selected`/`go_to_parent`
//! below reach back into it (`explorer::ensure_visible`/`request_dir`) for
//! the pieces `handle_dir_loaded` also needs to share.

use crate::app::App;
use crate::explorer::{self, ensure_visible};
use crate::keymap::{Binding, KeyCode, KeyInput, KeyOutcome, KeyPattern, Mods, resolve_in};
use crate::runtime::Effects;
use crate::workspace;

/// The Explorer's own commands (plan WP4.S3) — resolved via `EXPLORER_
/// BINDINGS`, mirroring `keymap::GlobalCommand`'s shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExplorerCommand {
    Up,
    Down,
    Top,
    Bottom,
    Open,
    ParentDir,
}

/// Arrow keys move one entry; Home/End jump to the ends; Enter opens the
/// selected entry (a file activates it, a directory navigates into it);
/// Backspace navigates to the parent of the CURRENT root (not the selected
/// entry) — mirroring Go filetree's `..`-less parent-dir chord.
pub const EXPLORER_BINDINGS: &[Binding<ExplorerCommand>] = &[
    Binding {
        keys: &[KeyPattern::new(KeyCode::Up, Mods::NONE)],
        cmd: ExplorerCommand::Up,
        help: "up",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Down, Mods::NONE)],
        cmd: ExplorerCommand::Down,
        help: "down",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Home, Mods::NONE)],
        cmd: ExplorerCommand::Top,
        help: "top",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::End, Mods::NONE)],
        cmd: ExplorerCommand::Bottom,
        help: "bottom",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Enter, Mods::NONE)],
        cmd: ExplorerCommand::Open,
        help: "open",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Backspace, Mods::NONE)],
        cmd: ExplorerCommand::ParentDir,
        help: "up dir",
        when: "",
        alias: false,
    },
];

/// Stage 3 of the four-stage key pipeline (plan Context, decision 8) when
/// `app.focus == Pane::Explorer`. `effects` is needed (unlike the plan's
/// literal `handle_key(app, key) -> KeyOutcome` sketch) because `Open`/
/// `ParentDir` must enqueue a `ReadDir` `Cmd` — a Vfs read can never run
/// inline in `update` (§5.4) — the same reason `app::handle_editor_key`
/// this mirrors already threads `effects` through for `Save`/clipboard.
pub fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    let Some(cmd) = resolve_in(EXPLORER_BINDINGS, key) else {
        return KeyOutcome::Ignored;
    };
    match cmd {
        ExplorerCommand::Up => move_selection(app, -1),
        ExplorerCommand::Down => move_selection(app, 1),
        ExplorerCommand::Top => {
            app.explorer.nav.first();
            ensure_visible(app);
        }
        ExplorerCommand::Bottom => {
            let len = app.explorer.entries.len();
            app.explorer.nav.last(len);
            ensure_visible(app);
        }
        ExplorerCommand::Open => open_selected(app, effects),
        ExplorerCommand::ParentDir => go_to_parent(app, effects),
    }
    KeyOutcome::Consumed
}

fn move_selection(app: &mut App, delta: isize) {
    let len = app.explorer.entries.len();
    app.explorer.nav.move_by(delta, len);
    ensure_visible(app);
}

/// Opens the currently selected entry: a file activates it through
/// `workspace::open_path`; a directory issues a `ReadDir` `Cmd` navigating
/// the Explorer into it (plan WP4.S3: "Open on a file → workspace::
/// open_path; Open on a dir → dir load Cmd for the new root"). The target
/// path comes straight from `entry.path` — the byte-exact path `Vfs::
/// read_dir` returned (plan WP13.S1) — never rejoined from `entry.name`
/// onto `app.explorer.root`: `name` is lossy-decoded for display, and
/// rejoining it would let a byte the user's filename actually has silently
/// become U+FFFD in the path the app opens (§0). The directory branch
/// resolves the candidate root through `app.vfs.resolve` first (§1.4.9),
/// same as `initial_root`/`open_path` already do — a plain `join` would let
/// an unresolved (e.g. symlinked) path become the Explorer's new root,
/// unlike every other root-changing path in this module. Falls back to the
/// unresolved path on a `resolve` error, mirroring `workspace::open_path`'s
/// own `unwrap_or_else` fallback (Prime Directive: a resolve failure must
/// never just strand the user mid-navigation).
fn open_selected(app: &mut App, effects: &mut Effects) {
    let Some((target, is_dir)) = app
        .explorer
        .entries
        .get(app.explorer.nav.cursor)
        .map(|e| (e.path.clone(), e.is_dir))
    else {
        return;
    };
    if is_dir {
        let resolved = app.vfs.resolve(&target).unwrap_or_else(|_| target.clone());
        explorer::request_dir(app, resolved, effects);
    } else {
        // `open_path` reports a read failure through `banner::report_error`
        // itself before returning `None` — discarding the `Option` here
        // drops only the opened id, never an unsurfaced error.
        let _ = workspace::open_path(app, &target);
    }
}

/// Backspace navigates to the CURRENT root's own parent — a no-op at a
/// filesystem root (`Path::parent` returns `None`), never a Cmd for a
/// nonexistent target. Resolved through `app.vfs.resolve` before use (see
/// `open_selected`'s docs) — a plain `Path::parent` is pure path arithmetic
/// that never consults the filesystem, unlike `initial_root`'s own root
/// resolution.
fn go_to_parent(app: &mut App, effects: &mut Effects) {
    let Some(parent) = app.explorer.root.parent() else {
        return;
    };
    let parent = parent.to_path_buf();
    let resolved = app.vfs.resolve(&parent).unwrap_or_else(|_| parent.clone());
    explorer::request_dir(app, resolved, effects);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::{DirEntry, Mem};

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
                is_dir: *is_dir,
            })
            .collect()
    }

    #[test]
    fn up_and_down_clamp_at_the_list_bounds() {
        let mut app = app();
        explorer::handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("b", false), ("c", false)]),
            DirCause::Nav,
            0,
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
