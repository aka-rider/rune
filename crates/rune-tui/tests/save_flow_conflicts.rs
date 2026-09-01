//! The direct-vfs-fallback conflict-sibling save tests — split out of
//! `save_flow.rs` to keep that file under the file-size ceiling, the same
//! shape `save_flow.rs` itself already uses for `src/save.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod save_flow_common;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::undo::EditKind;
use rune_tui::app::App;
use rune_tui::commands::edit;
use rune_vfs::{DirEntry, Mem, Stat, Vfs, VfsTestExt};
use save_flow_common::{press_save, settle_cmds};

/// The direct-vfs fallback (no store binding) has no CAS baseline of its
/// own, so `rune_vfs::put`'s `Force { expect: None }` conservatively flags
/// whatever content it displaces as `Raced` — here, simply this document's
/// own prior on-disk bytes, the ordinary shape of ANY edit-then-save. With
/// no recovery store to hand those displaced bytes to, the fallback must
/// preserve them itself (a durable sibling file next to `path`) and tell
/// the user, rather than the old behavior of silently discarding them once
/// the save collapsed `Committed`/`Raced` into the same bare success.
#[test]
fn a_direct_save_that_displaces_existing_disk_content_preserves_it_and_warns() {
    let vfs = Arc::new(Mem::new());
    let path = PathBuf::from("/doc.md");
    vfs.save_atomic(&path, b"already on disk")
        .expect("seed doc.md");
    let mut app = App::new(
        Buffer::new("already on disk"),
        Some(
            rune_tui::resolved::ResolvedPath::resolve(
                vfs.as_ref(),
                std::path::Path::new(&path.clone()),
            )
            .expect("the launch path resolves"),
        ),
        Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
        None,
    );
    let id = app.active;
    edit::insert_text(&mut app, id, "!", EditKind::Insert);
    assert!(app.is_dirty(), "the fixture must actually be dirty");

    let effects = press_save(&mut app);
    settle_cmds(&mut app, effects);

    assert!(!app.is_dirty(), "the save must still succeed");
    assert_eq!(vfs.read(&path).unwrap(), b"!already on disk");

    let log = rune_tui::messages::log_text(&app);
    assert!(
        log.contains("kept at"),
        "the displaced bytes must be reported preserved, not dropped silently: {log:?}"
    );

    let siblings: Vec<PathBuf> = vfs
        .debug_paths()
        .into_iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().contains("doc.md.conflict-"))
        })
        .collect();
    assert_eq!(
        siblings.len(),
        1,
        "exactly one conflict sibling must hold the displaced bytes: {siblings:?}"
    );
    assert_eq!(
        vfs.read(&siblings[0]).unwrap(),
        b"already on disk",
        "the sibling must hold exactly what the save displaced"
    );
}

/// A `Vfs` that fails `write_durable` for any path naming a conflict
/// sibling (`preserve_displaced`'s own naming scheme) while forwarding
/// every other call to a real `Mem` — models a disk that has room for the
/// primary save but not for the extra sibling file.
struct FailConflictSiblingWritesVfs {
    inner: Mem,
}

impl Vfs for FailConflictSiblingWritesVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        if path.to_string_lossy().contains(".conflict-") {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "no room for a conflict sibling",
            ));
        }
        self.inner.write_durable(path, bytes)
    }
    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        self.inner.exchange(a, b)
    }
    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()> {
        self.inner.rename_excl(old, new)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.inner.remove(path)
    }
    fn trash(&self, path: &Path) -> io::Result<()> {
        self.inner.trash(path)
    }
    fn stat(&self, path: &Path) -> io::Result<Stat> {
        self.inner.stat(path)
    }
    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.read_link(path)
    }
    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.resolve(path)
    }
    fn mkdir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.mkdir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.inner.read_dir(path)
    }
}

/// When the primary save succeeds but the extra sibling write for the
/// displaced bytes fails, the save must still report success (the user's
/// own edit is safe on disk) while clearly warning that the displaced
/// content could NOT be preserved — never silently losing that fact.
#[test]
fn a_direct_save_whose_conflict_sibling_write_fails_still_saves_but_warns() {
    let vfs = Arc::new(FailConflictSiblingWritesVfs { inner: Mem::new() });
    let path = PathBuf::from("/doc.md");
    vfs.save_atomic(&path, b"already on disk")
        .expect("seed doc.md");
    let mut app = App::new(
        Buffer::new("already on disk"),
        Some(
            rune_tui::resolved::ResolvedPath::resolve(
                vfs.as_ref(),
                std::path::Path::new(&path.clone()),
            )
            .expect("the launch path resolves"),
        ),
        Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
        None,
    );
    let id = app.active;
    edit::insert_text(&mut app, id, "!", EditKind::Insert);

    let effects = press_save(&mut app);
    settle_cmds(&mut app, effects);

    assert!(
        !app.is_dirty(),
        "the primary save must still succeed even when the sibling write fails"
    );
    assert_eq!(vfs.read(&path).unwrap(), b"!already on disk");

    let log = rune_tui::messages::log_text(&app);
    assert!(
        log.contains("could not be preserved"),
        "a failed preservation attempt must be surfaced, never swallowed: {log:?}"
    );
}
