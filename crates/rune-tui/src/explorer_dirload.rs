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

    crate::explorer_search::clear_search(app);
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
    fn refresh_falls_back_to_the_top_when_the_selection_vanished() {
        let mut app = app();
        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("gone", false)]),
            DirCause::Nav,
            crate::generation::Generation::ZERO,
        );
        app.explorer.nav.cursor = 2;

        handle_dir_loaded(
            &mut app,
            PathBuf::from("/root"),
            entries(&[("a", false), ("still-here", false)]),
            DirCause::Refresh,
            crate::generation::Generation::ZERO,
        );
        assert_eq!(app.explorer.nav.cursor, 0);
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
