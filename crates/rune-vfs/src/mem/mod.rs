//! In-memory `Vfs` for tests.
//!
//! A `Mutex`-backed `HashMap<PathBuf, MemFile>` with synthetic inodes (so
//! `Vfs::exchange`/`rename_excl` can be verified to move identity the same
//! way `Disk`'s `renamex_np` does) and an optional one-shot `fail_next`
//! injection scoped to a specific `OpKind`.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};

use crate::path_util::{classify, follow_links, kind_at, lexically_normalize, not_found};
use crate::{DirEntry, FileKind, Identity, Link, Stat, Vfs, sort_dir_entries, temp_name};

#[cfg(any(test, feature = "fault-injection"))]
mod fault;
#[cfg(any(test, feature = "fault-injection"))]
pub use fault::OpKind;

pub(crate) struct MemFile {
    data: Vec<u8>,
    inode: u64,
    device: u64,
    mod_tick: u64,
    /// Hard-link count `Vfs::stat` reports. Defaults to 1 (Mem's own files
    /// are never actually hard-linked); settable via `Mem::set_nlink` so the
    /// hardlink-fork warning path (consumed by `rune-db` observation/load)
    /// has a test double capable of exercising `nlink > 1` (WP1.S6).
    nlink: u64,
    /// The `FileKind` `Vfs::stat` reports. Defaults to `File`; settable via
    /// `Mem::set_kind` so tests can model a FIFO/socket/device node
    /// (`FileKind::Other`) without a real filesystem.
    pub(crate) kind: FileKind,
    /// `Some` makes this entry a symlink naming that target — absolute, or
    /// relative to the link's own parent. Seeded by `Mem::symlink`.
    pub(crate) link_target: Option<PathBuf>,
}

struct MemState {
    files: HashMap<PathBuf, MemFile>,
    next_inode: u64,
    tick: u64,
}

/// In-memory `Vfs` keyed by `PathBuf`. Suitable for tests.
pub struct Mem {
    state: Mutex<MemState>,
    #[cfg(any(test, feature = "fault-injection"))]
    faults: fault::Faults,
}

impl Mem {
    pub fn new() -> Self {
        Mem {
            state: Mutex::new(MemState {
                files: HashMap::new(),
                next_inode: 1,
                tick: 0,
            }),
            #[cfg(any(test, feature = "fault-injection"))]
            faults: fault::Faults::new(),
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, MemState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for Mem {
    fn default() -> Self {
        Self::new()
    }
}

impl Vfs for Mem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        #[cfg(any(test, feature = "fault-injection"))]
        self.take_failure(OpKind::Read)?;
        let state = self.lock_state();
        state
            .files
            .get(path)
            .map(|f| f.data.clone())
            .ok_or_else(|| not_found(path, "read"))
    }

    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        #[cfg(any(test, feature = "fault-injection"))]
        self.take_failure(OpKind::WriteDurable)?;
        let temp = temp_name(path);
        let mut state = self.lock_state();
        // Backend parity with `Disk::write_durable` (`OpenOptions::
        // create_new(true)`, which errors `AlreadyExists` rather than
        // silently truncating a colliding temp): a `HashMap::insert` here
        // would instead silently overwrite whatever the collision already
        // held, making that failure mode untestable against `Mem`.
        if state.files.contains_key(&temp) {
            return Err(crate::wrap_io(
                io::Error::new(io::ErrorKind::AlreadyExists, "temp already exists"),
                format!("write_durable {}", temp.display()),
            ));
        }
        state.tick += 1;
        let mod_tick = state.tick;
        let inode = state.next_inode;
        state.next_inode += 1;
        state.files.insert(
            temp.clone(),
            MemFile {
                data: bytes.to_vec(),
                inode,
                device: 1,
                mod_tick,
                nlink: 1,
                kind: FileKind::File,
                link_target: None,
            },
        );
        drop(state);
        Ok(temp)
    }

    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        #[cfg(any(test, feature = "fault-injection"))]
        self.take_failure(OpKind::Exchange)?;
        let mut state = self.lock_state();
        if a == b {
            // Matches `Disk`'s `renamex_np(RENAME_SWAP)` semantics: swapping
            // a path with itself is a no-op success, never a delete (WP1.S3
            // — the previous eager-double-remove below silently dropped the
            // file in exactly this case, since the second `remove` of the
            // SAME key always missed).
            return if state.files.contains_key(a) {
                Ok(())
            } else {
                Err(not_found(a, "exchange"))
            };
        }
        // Every `remove` from here on is paired with an `insert` before the
        // function returns on every path (the swap, or a restore on the
        // second key's miss) — an un-reinserted removal is unrepresentable.
        let Some(mut fa) = state.files.remove(a) else {
            return Err(not_found(a, "exchange"));
        };
        let Some(mut fb) = state.files.remove(b) else {
            state.files.insert(a.to_path_buf(), fa);
            return Err(not_found(b, "exchange"));
        };
        state.tick += 1;
        let mod_tick = state.tick;
        fa.mod_tick = mod_tick;
        fb.mod_tick = mod_tick;
        state.files.insert(a.to_path_buf(), fb);
        state.files.insert(b.to_path_buf(), fa);
        drop(state);
        #[cfg(any(test, feature = "fault-injection"))]
        if let Some(e) = self.take_after_failure(
            OpKind::Exchange,
            format!("exchange {} <-> {}", a.display(), b.display()),
        ) {
            return Err(e);
        }
        Ok(())
    }

    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()> {
        #[cfg(any(test, feature = "fault-injection"))]
        self.take_failure(OpKind::RenameExcl)?;
        let mut state = self.lock_state();
        if !state.files.contains_key(old) {
            return Err(not_found(old, "renameexcl"));
        }
        if state.files.contains_key(new) {
            return Err(crate::wrap_io(
                io::Error::new(io::ErrorKind::AlreadyExists, "destination exists"),
                format!("renameexcl {} -> {}", old.display(), new.display()),
            ));
        }
        // Confirmed present above under this same (still-held) lock, so
        // this cannot miss.
        let Some(f) = state.files.remove(old) else {
            return Err(not_found(old, "renameexcl"));
        };
        state.files.insert(new.to_path_buf(), f);
        drop(state);
        #[cfg(any(test, feature = "fault-injection"))]
        if let Some(e) = self.take_after_failure(
            OpKind::RenameExcl,
            format!("renameexcl {} -> {}", old.display(), new.display()),
        ) {
            return Err(e);
        }
        Ok(())
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        #[cfg(any(test, feature = "fault-injection"))]
        self.take_failure(OpKind::Remove)?;
        let mut state = self.lock_state();
        if state.files.remove(path).is_none() {
            return Err(not_found(path, "remove"));
        }
        drop(state);
        Ok(())
    }

    fn trash(&self, path: &Path) -> io::Result<()> {
        #[cfg(any(test, feature = "fault-injection"))]
        self.take_failure(OpKind::Trash)?;
        let mut state = self.lock_state();
        if state.files.remove(path).is_none() {
            return Err(not_found(path, "trash"));
        }
        drop(state);
        Ok(())
    }

    fn stat(&self, path: &Path) -> io::Result<Stat> {
        #[cfg(any(test, feature = "fault-injection"))]
        self.take_failure(OpKind::Stat)?;
        let result = {
            let state = self.lock_state();
            let resolved = follow_links(&state.files, path)?;
            state.files.get(&resolved).map(|f| {
                Ok(Stat {
                    size: f.data.len() as u64,
                    mtime: UNIX_EPOCH + Duration::from_millis(f.mod_tick),
                    identity: Identity {
                        inode: Some(f.inode),
                        device: Some(f.device),
                    },
                    // Mem has no real hard-link mechanism; the count is just
                    // whatever `Mem::set_nlink` last set for this path
                    // (defaulting to 1), so a test can drive the hardlink-fork
                    // warning path (WP1.S6).
                    nlink: Some(f.nlink),
                    kind: f.kind,
                })
            })
        };
        if let Some(result) = result {
            #[cfg(any(test, feature = "fault-injection"))]
            self.apply_pending_mutation(path);
            return result;
        }
        // No exact file at `path` — `Mem` has no directory nodes, so a
        // directory is synthesized: `path` is a directory iff some stored
        // key sits strictly below it.
        let state = self.lock_state();
        let resolved = follow_links(&state.files, path)?;
        let is_synthetic_dir = kind_at(&state.files, &resolved) == Some(FileKind::Dir);
        drop(state);
        if is_synthetic_dir {
            return Ok(Stat {
                size: 0,
                mtime: UNIX_EPOCH,
                identity: Identity::default(),
                nlink: None,
                kind: FileKind::Dir,
            });
        }
        Err(not_found(path, "stat"))
    }

    /// Lexical normalization only — no symlinks, no real filesystem access
    /// (WP1.S6). `Mem` has no directory tree to canonicalize against, so
    /// this is the purely-textual half of what `Disk::resolve`'s
    /// `fs::canonicalize` does: collapse `.`/`..` components and anchor a
    /// relative path at Mem's own synthetic root, so two spellings of the
    /// same target (`/a/./b.md`, `/a/x/../b.md`, `b.md` vs `/b.md`) become
    /// the same `HashMap` key instead of two unrelated ones.
    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        #[cfg(any(test, feature = "fault-injection"))]
        self.take_failure(OpKind::Resolve)?;
        let normalized = lexically_normalize(path);
        #[cfg(any(test, feature = "fault-injection"))]
        {
            let failing = self
                .faults
                .resolve_failures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if failing.contains(&normalized) {
                return Err(crate::wrap_io(
                    io::Error::other("fail_resolve triggered"),
                    format!("resolve {}", normalized.display()),
                ));
            }
        }
        let state = self.lock_state();
        let followed = follow_links(&state.files, &normalized)?;
        drop(state);
        Ok(lexically_normalize(&followed))
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        #[cfg(any(test, feature = "fault-injection"))]
        self.take_failure(OpKind::ReadLink)?;
        let normalized = lexically_normalize(path);
        let state = self.lock_state();
        if let Some(target) = state
            .files
            .get(&normalized)
            .and_then(|f| f.link_target.clone())
        {
            return Ok(target);
        }
        let exists = kind_at(&state.files, &normalized).is_some();
        drop(state);
        if exists {
            return Err(crate::wrap_io(
                io::Error::new(io::ErrorKind::InvalidInput, "not a symlink"),
                format!("read_link {}", path.display()),
            ));
        }
        Err(not_found(path, "read_link"))
    }

    /// No-op: Mem has no directory tree, only flat path->content keys.
    fn mkdir_all(&self, _path: &Path) -> io::Result<()> {
        #[cfg(any(test, feature = "fault-injection"))]
        self.take_failure(OpKind::MkdirAll)?;
        Ok(())
    }

    /// `Mem` has no directory nodes (`MemState.files` is a flat
    /// `HashMap<PathBuf, MemFile>`), so children are derived from key
    /// shape: for every key under `path`, the component immediately below
    /// `path` becomes an entry. A key exactly one component deeper is a
    /// file; anything deeper contributes its first component as a
    /// synthetic directory. A key not under `path` at all is not a
    /// component-prefix match, so it's skipped.
    ///
    /// Exactly ONE entry per name, even when a name is claimed BOTH ways —
    /// once as a synthetic directory (some key goes deeper below it) and
    /// once as an exact file key itself (e.g. `path/a` stored as a file AND
    /// `path/a/b.md` stored too, an inconsistent-but-representable `Mem`
    /// state): the directory claim always wins, contributing `FileKind::
    /// Dir`, and the file claim for the same name is dropped rather than
    /// contributing a second entry. Every key under `path` is folded into a
    /// `name -> kind` map first (dir overwrites file, never the reverse; a
    /// HashMap's iteration order can visit either key first) and only THEN
    /// turned into `entries`, so the result never depends on which of the
    /// colliding keys `state.files` happens to iterate first.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        #[cfg(any(test, feature = "fault-injection"))]
        self.take_failure(OpKind::ReadDir)?;
        let state = self.lock_state();
        let listed = follow_links(&state.files, path)?;
        // WP13.S1: the `PathBuf` travels alongside `kind` in the same fold,
        // built from `path.join(first)` — the byte-exact `Component`
        // straight off the stored key, never round-tripped through the
        // lossy `String` `name` also computed below.
        let mut by_name: HashMap<String, (FileKind, Link, PathBuf)> = HashMap::new();
        // WP1.S6: `Disk::read_dir` on a nonexistent path errors `NotFound`;
        // `Mem` used to report an empty listing instead, since it derives
        // everything from key shape and a path with zero matching keys
        // looked identical to a genuinely empty (but existing) directory.
        // The synthetic root always exists; any other path needs either an
        // exact key (it's a stored file) or at least one key nested below
        // it (it's a synthetic directory) to count as present.
        let mut path_exists = listed == Path::new("/") || state.files.contains_key(&listed);
        for key in state.files.keys() {
            let Ok(rest) = key.strip_prefix(&listed) else {
                continue;
            };
            let Some(first) = rest.components().next() else {
                // `rest` is empty: `key == listed`, not a child of it.
                continue;
            };
            path_exists = true;
            let name = first.as_os_str().to_string_lossy().to_string();
            let (kind, link) = classify(&state.files, &listed.join(first.as_os_str()));
            by_name
                .entry(name)
                .or_insert((kind, link, path.join(first.as_os_str())));
        }
        if !path_exists {
            return Err(not_found(path, "read_dir"));
        }
        drop(state);
        let mut entries: Vec<DirEntry> = by_name
            .into_iter()
            .map(|(name, (kind, link, path))| DirEntry {
                name,
                path,
                kind,
                link,
            })
            .collect();
        sort_dir_entries(&mut entries);
        Ok(entries)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::testing::VfsTestExt;

    /// `stat` and `read_dir` derive "is this a synthetic directory" from the
    /// same predicate (`sits_strictly_below`); this exercises both entry
    /// points against the same fixture to prove they agree at every level —
    /// the file itself, its immediate parent, and an ancestor two levels up.
    #[test]
    fn stat_and_read_dir_agree_on_synthetic_directories() {
        let vfs = Mem::new();
        vfs.save_atomic(Path::new("/a/b/c.md"), b"content").unwrap();

        let stat_a = vfs.stat(Path::new("/a")).unwrap();
        assert_eq!(stat_a.kind, FileKind::Dir);
        assert!(vfs.read_dir(Path::new("/a")).is_ok());

        let stat_ab = vfs.stat(Path::new("/a/b")).unwrap();
        assert_eq!(stat_ab.kind, FileKind::Dir);
        assert!(vfs.read_dir(Path::new("/a/b")).is_ok());

        let stat_file = vfs.stat(Path::new("/a/b/c.md")).unwrap();
        assert_eq!(stat_file.kind, FileKind::File);
        // A file key with no descendants below it lists no children — same
        // shape `mem_read_dir_excludes_the_queried_path_itself` already
        // relies on — rather than erroring the way `Disk::read_dir` would
        // on a real non-directory path (`Mem` has no directory nodes to
        // distinguish "file" from "empty directory" by).
        assert_eq!(
            vfs.read_dir(Path::new("/a/b/c.md")).unwrap(),
            Vec::<DirEntry>::new()
        );
    }

    #[test]
    fn trash_removes_a_written_file() {
        let vfs = Mem::new();
        vfs.save_atomic(Path::new("/a/b.md"), b"content").unwrap();

        vfs.trash(Path::new("/a/b.md")).unwrap();

        assert_eq!(
            vfs.read(Path::new("/a/b.md")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn trash_of_a_missing_path_errors() {
        let vfs = Mem::new();

        assert!(vfs.trash(Path::new("/missing.md")).is_err());
    }

    #[test]
    fn successive_write_durables_mint_distinct_identity_and_mod_tick() {
        let vfs = Mem::new();

        let t1 = vfs.write_durable(Path::new("/a"), b"1").unwrap();
        let t2 = vfs.write_durable(Path::new("/b"), b"2").unwrap();

        let s1 = vfs.stat(&t1).unwrap();
        let s2 = vfs.stat(&t2).unwrap();
        assert_ne!(
            s1.identity.inode, s2.identity.inode,
            "each write_durable must mint a fresh inode"
        );
        assert_ne!(
            s1.mtime, s2.mtime,
            "each write_durable must advance the shared mod tick"
        );
    }

    #[test]
    fn exchange_advances_the_shared_mod_tick_past_both_files_prior_values() {
        let vfs = Mem::new();
        vfs.save_atomic(Path::new("/a"), b"1").unwrap();
        vfs.save_atomic(Path::new("/b"), b"2").unwrap();
        let before_a = vfs.stat(Path::new("/a")).unwrap().mtime;
        let before_b = vfs.stat(Path::new("/b")).unwrap().mtime;

        vfs.exchange(Path::new("/a"), Path::new("/b")).unwrap();

        let after_a = vfs.stat(Path::new("/a")).unwrap().mtime;
        let after_b = vfs.stat(Path::new("/b")).unwrap().mtime;
        assert!(after_a > before_a && after_a > before_b);
        assert_eq!(
            after_a, after_b,
            "the swap must stamp both files with the same, newly advanced tick"
        );
    }

    #[test]
    fn stat_mtime_advances_forward_from_the_epoch() {
        let vfs = Mem::new();
        vfs.save_atomic(Path::new("/a"), b"1").unwrap();

        let mtime = vfs.stat(Path::new("/a")).unwrap().mtime;

        assert!(
            mtime > std::time::UNIX_EPOCH,
            "mod_tick must advance mtime forward from the epoch, never backward"
        );
    }

    #[test]
    fn fail_next_mkdir_all_makes_the_next_mkdir_all_call_fail() {
        let vfs = Mem::new();
        vfs.fail_next(OpKind::MkdirAll, io::ErrorKind::PermissionDenied);

        let err = vfs.mkdir_all(Path::new("/anything")).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn fail_next_trash_makes_the_next_trash_fail() {
        let vfs = Mem::new();
        vfs.save_atomic(Path::new("/a/b.md"), b"content").unwrap();
        vfs.fail_next(OpKind::Trash, io::ErrorKind::PermissionDenied);

        let err = vfs.trash(Path::new("/a/b.md")).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        // The failed attempt never touched state: the file is still there.
        assert!(vfs.read(Path::new("/a/b.md")).is_ok());
    }
}
