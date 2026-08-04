//! Reacts to `Msg::DirLoaded`, routed from `app::update_inner` through
//! `explorer::handle_dir_loaded` (a re-export of `handle_dir_loaded` below —
//! every other module keeps calling it through `explorer::`, unaware it
//! moved here per §1.6).

use std::path::{Path, PathBuf};

use rune_vfs::DirEntry;

use crate::app::App;
use crate::explorer::ensure_visible;
use crate::runtime::DirCause;

/// Prepends a synthetic `..` row to `entries` when `root` has a parent — a
/// REAL `DirEntry` carrying the real parent path, not a render-time overlay.
/// Because it's a genuine list element, `open_selected`'s existing
/// directory branch (resolve, then `request_dir`) already does exactly what
/// `go_to_parent` does when the user presses Enter on it — no `".."`
/// special case anywhere, and `listnav::List`'s cursor keeps addressing the
/// one real list it's always addressed, never an N+1 rendered one. A root
/// with no parent (a filesystem root) gets no such row.
fn with_parent_entry(root: &Path, mut entries: Vec<DirEntry>) -> Vec<DirEntry> {
    let Some(parent) = root.parent() else {
        return entries;
    };
    entries.insert(
        0,
        DirEntry {
            name: "..".to_string(),
            path: parent.to_path_buf(),
            is_dir: true,
        },
    );
    entries
}

/// A `generation` that no longer matches `app.explorer.request_generation`
/// is a reply to a SUPERSEDED request (a later `ReadDir` was already
/// issued — `request_dir`/the initial `^x` load bump the generation at
/// every issue site) and is ignored outright, never adopted over whatever
/// a newer, still-in-flight (or already-landed) request produced. `Nav`
/// always adopts the new root/entries and resets the cursor to the top;
/// `Refresh` keeps the currently selected entry selected BY NAME when it's
/// still present in the new listing (falling back to the top otherwise) —
/// this is exactly what `refresh_for` (`explorer.rs`) issues on every
/// successful rename that lands inside the current Explorer root, so the
/// branch has a real production caller, not just this file's own tests;
/// it's also the shape a later fsnotify-driven reload would use. A pending
/// reveal (`Explorer::pending_reveal`, set by `explorer_reveal::reveal`)
/// wins over both, landing the cursor on its own target instead.
pub(crate) fn handle_dir_loaded(
    app: &mut App,
    root: PathBuf,
    entries: Vec<DirEntry>,
    cause: DirCause,
    generation: u32,
) {
    if generation != app.explorer.request_generation {
        return;
    }

    crate::explorer_search::clear_search(app); // a new listing outdates any query
    let entries = with_parent_entry(&root, entries);

    let reveal_target = app.explorer.pending_reveal.take();
    let preserve_name = match cause {
        DirCause::Nav => None,
        DirCause::Refresh => app
            .explorer
            .entries
            .get(app.explorer.nav.cursor)
            .map(|e| e.name.clone()),
    };

    app.explorer.root = root;
    app.explorer.entries = entries;
    app.explorer.loading = false;
    let by_reveal =
        reveal_target.and_then(|t| app.explorer.entries.iter().position(|e| e.path == t));
    let by_name = preserve_name.and_then(|n| app.explorer.entries.iter().position(|e| e.name == n));
    app.explorer.nav.cursor = by_reveal.or(by_name).unwrap_or(0);
    ensure_visible(app);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

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
    fn nav_load_resets_the_cursor_to_the_top() {
        let mut app = app();
        app.explorer.nav.cursor = 3;
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("b", false)]),
            DirCause::Nav,
            0,
        );
        assert_eq!(app.explorer.nav.cursor, 0);
        // "/root" has a parent ("/"), so a synthetic ".." row is prepended.
        assert_eq!(app.explorer.entries.len(), 3);
    }

    #[test]
    fn refresh_preserves_the_selected_entry_by_name() {
        let mut app = app();
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("b", false), ("c", false)]),
            DirCause::Nav,
            0,
        );
        app.explorer.nav.cursor = 3; // "c", shifted one place by the leading ".." row

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("new", false), ("a", false), ("c", false)]),
            DirCause::Refresh,
            0,
        );
        assert_eq!(app.explorer.entries[app.explorer.nav.cursor].name, "c");
    }

    #[test]
    fn refresh_falls_back_to_the_top_when_the_selection_vanished() {
        let mut app = app();
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("gone", false)]),
            DirCause::Nav,
            0,
        );
        app.explorer.nav.cursor = 2; // "gone", shifted one place by the leading ".." row

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("still-here", false)]),
            DirCause::Refresh,
            0,
        );
        assert_eq!(app.explorer.nav.cursor, 0);
    }

    /// A `DirLoaded` reply whose `generation` no longer matches the
    /// Explorer's current `request_generation` (a later request already
    /// superseded it) must be ignored outright — the review fix for two
    /// in-flight `ReadDir` Cmds landing out of order.
    #[test]
    fn a_stale_generation_reply_is_ignored() {
        let mut app = app();
        app.explorer.request_generation = 5;
        app.explorer.root = PathBuf::from("/root");
        app.explorer.entries = entries(&[("a", false)]);

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/elsewhere"),
            entries(&[("stale", false)]),
            DirCause::Nav,
            4, // superseded — the live generation is 5
        );

        assert_eq!(
            app.explorer.root,
            PathBuf::from("/root"),
            "a stale-generation reply must not overwrite the current listing"
        );
        assert_eq!(app.explorer.entries, entries(&[("a", false)]));
    }

    /// The reply carrying the CURRENT generation is applied normally.
    #[test]
    fn the_current_generation_reply_is_applied() {
        let mut app = app();
        app.explorer.request_generation = 5;

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/fresh"),
            entries(&[("fresh", false)]),
            DirCause::Nav,
            5,
        );

        assert_eq!(app.explorer.root, PathBuf::from("/fresh"));
        // "/fresh" has a parent ("/"), so a synthetic ".." row leads the list.
        let mut expected = entries(&[("fresh", false)]);
        expected.insert(
            0,
            DirEntry {
                name: "..".to_string(),
                path: PathBuf::from("/"),
                is_dir: true,
            },
        );
        assert_eq!(app.explorer.entries, expected);
    }
}
