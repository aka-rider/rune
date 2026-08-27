use std::path::{Path, PathBuf};

use rune_vfs::{DirEntry, FileKind, Link};

use crate::app::App;
use crate::explorer::ensure_visible;
use crate::runtime::DirCause;

fn with_parent_entry(root: &Path, mut entries: Vec<DirEntry>) -> Vec<DirEntry> {
    let Some(parent) = root.parent() else {
        return entries;
    };
    entries.insert(
        0,
        DirEntry {
            name: "..".to_string(),
            path: parent.to_path_buf(),
            kind: FileKind::Dir,
            link: Link::No,
        },
    );
    entries
}

/// The neighbours of a `Refresh`'s previously-selected row, captured before
/// the fresh listing replaces `app.explorer.entries` — a background
/// completion (trash-done, rename-done) reloads the SAME directory the user
/// is already looking at, so the selection should follow the row it was on:
/// still there under the same name (`current`), or — if that row is the one
/// that just vanished — the nearest surviving neighbour, preferring the row
/// that was AFTER it (the standard file-manager convention) and falling
/// back to the row BEFORE when the vanished row was last.
struct RefreshAnchor {
    current: Option<String>,
    next: Option<String>,
    prev: Option<String>,
}

impl RefreshAnchor {
    fn capture(entries: &[DirEntry], cursor: usize) -> RefreshAnchor {
        RefreshAnchor {
            current: entries.get(cursor).map(|e| e.name.clone()),
            next: entries.get(cursor + 1).map(|e| e.name.clone()),
            prev: cursor
                .checked_sub(1)
                .and_then(|i| entries.get(i))
                .map(|e| e.name.clone()),
        }
    }

    fn resolve(&self, entries: &[DirEntry]) -> Option<usize> {
        let find = |name: &Option<String>| {
            name.as_deref()
                .and_then(|n| entries.iter().position(|e| e.name == n))
        };
        find(&self.current)
            .or_else(|| find(&self.next))
            .or_else(|| find(&self.prev))
    }
}

pub(crate) fn handle_dir_loaded(
    app: &mut App,
    root: PathBuf,
    entries: Vec<DirEntry>,
    cause: DirCause,
    generation: crate::generation::DirLoadGen,
) {
    if generation != app.explorer.request_generation {
        return;
    }

    // Only a real navigation invalidates an in-progress type-to-search: a
    // `Refresh` reloads the directory the user is already looking at from a
    // background completion they didn't ask for (trash-done, rename-done),
    // so whatever they were typing is still describing something on THIS
    // screen and must survive it.
    if matches!(cause, DirCause::Nav) {
        crate::explorer_search::clear_search(app);
    }
    let entries = with_parent_entry(&root, entries);

    let reveal_target = app.explorer.pending_reveal.take();
    let anchor = match cause {
        DirCause::Nav => None,
        DirCause::Refresh => Some(RefreshAnchor::capture(
            &app.explorer.entries,
            app.explorer.nav.cursor,
        )),
    };

    app.explorer.root = root;
    app.explorer.entries = entries;
    app.explorer.loading = false;
    let by_reveal =
        reveal_target.and_then(|t| app.explorer.entries.iter().position(|e| e.path == t));
    let by_anchor = anchor.and_then(|a| a.resolve(&app.explorer.entries));
    app.explorer.nav.cursor = by_reveal.or(by_anchor).unwrap_or(0);
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
    fn nav_load_resets_the_cursor_to_the_top() {
        let mut app = app();
        app.explorer.nav.cursor = 3;
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("b", false)]),
            DirCause::Nav,
            crate::generation::Generation::ZERO,
        );
        assert_eq!(app.explorer.nav.cursor, 0);
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
            crate::generation::Generation::ZERO,
        );
        app.explorer.nav.cursor = 3;

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("new", false), ("a", false), ("c", false)]),
            DirCause::Refresh,
            crate::generation::Generation::ZERO,
        );
        assert_eq!(app.explorer.entries[app.explorer.nav.cursor].name, "c");
    }

    #[test]
    fn refresh_selects_the_row_after_when_the_selected_entry_vanished_from_the_middle() {
        let mut app = app();
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("gone", false), ("z", false)]),
            DirCause::Nav,
            crate::generation::Generation::ZERO,
        );
        app.explorer.nav.cursor = 2; // "gone"

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("z", false)]),
            DirCause::Refresh,
            crate::generation::Generation::ZERO,
        );
        assert_eq!(
            app.explorer.entries[app.explorer.nav.cursor].name, "z",
            "the row after the vanished entry is the standard file-manager landing spot"
        );
    }

    #[test]
    fn refresh_selects_the_row_before_when_the_vanished_entry_was_last() {
        let mut app = app();
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("gone", false)]),
            DirCause::Nav,
            crate::generation::Generation::ZERO,
        );
        app.explorer.nav.cursor = 2; // "gone", the last row

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false)]),
            DirCause::Refresh,
            crate::generation::Generation::ZERO,
        );
        assert_eq!(
            app.explorer.entries[app.explorer.nav.cursor].name, "a",
            "no row after survives, so the row before is the fallback"
        );
    }

    #[test]
    fn nav_clears_an_in_progress_search() {
        let mut app = app();
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false)]),
            DirCause::Nav,
            crate::generation::Generation::ZERO,
        );
        app.explorer_find_push('a');
        assert!(app.explorer_find().is_some(), "test setup: search armed");

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/elsewhere"),
            entries(&[("b", false)]),
            DirCause::Nav,
            crate::generation::Generation::ZERO,
        );
        assert_eq!(
            app.explorer_find(),
            None,
            "a real navigation invalidates a search typed against the old listing"
        );
    }

    #[test]
    fn a_background_refresh_leaves_an_in_progress_search_alone() {
        let mut app = app();
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("b", false)]),
            DirCause::Nav,
            crate::generation::Generation::ZERO,
        );
        app.explorer_find_push('b');
        assert!(app.explorer_find().is_some(), "test setup: search armed");

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("b", false)]),
            DirCause::Refresh,
            crate::generation::Generation::ZERO,
        );
        assert_eq!(
            app.explorer_find(),
            Some("b"),
            "a background completion's own refresh must not wipe a query the user is mid-typing"
        );
    }

    #[test]
    fn a_stale_generation_reply_is_ignored() {
        let mut app = app();
        app.explorer.request_generation = crate::generation::Generation::from_raw(5);
        app.explorer.root = PathBuf::from("/root");
        app.explorer.entries = entries(&[("a", false)]);

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/elsewhere"),
            entries(&[("stale", false)]),
            DirCause::Nav,
            crate::generation::Generation::from_raw(4),
        );

        assert_eq!(
            app.explorer.root,
            PathBuf::from("/root"),
            "a stale-generation reply must not overwrite the current listing"
        );
        assert_eq!(app.explorer.entries, entries(&[("a", false)]));
    }

    #[test]
    fn the_current_generation_reply_is_applied() {
        let mut app = app();
        app.explorer.request_generation = crate::generation::Generation::from_raw(5);

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/fresh"),
            entries(&[("fresh", false)]),
            DirCause::Nav,
            crate::generation::Generation::from_raw(5),
        );

        assert_eq!(app.explorer.root, PathBuf::from("/fresh"));
        let mut expected = entries(&[("fresh", false)]);
        expected.insert(
            0,
            DirEntry {
                name: "..".to_string(),
                path: PathBuf::from("/"),
                kind: FileKind::Dir,
                link: rune_vfs::Link::No,
            },
        );
        assert_eq!(app.explorer.entries, expected);
    }
}
