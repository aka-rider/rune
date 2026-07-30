//! In-memory `Vfs` for tests — mirrors Go's in-memory filesystem.
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

use crate::{DirEntry, FileKind, Identity, Stat, Vfs, sort_dir_entries, temp_name};

/// The `Vfs` operation a `Mem::fail_next`/`Mem::fail_after` injection
/// targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpKind {
    Read,
    WriteDurable,
    Exchange,
    RenameExcl,
    Remove,
    Stat,
    Resolve,
    MkdirAll,
    ReadDir,
}

struct MemFile {
    data: Vec<u8>,
    inode: u64,
    device: u64,
    mod_tick: u64,
    /// Hard-link count `Vfs::stat` reports. Defaults to 1 (Mem's own files
    /// are never actually hard-linked); settable via `Mem::set_nlink` so the
    /// hardlink-fork warning path (consumed by `rune-db` observation/load)
    /// has a test double capable of exercising `nlink > 1` (WP1.S6).
    nlink: u64,
}

struct MemState {
    files: HashMap<PathBuf, MemFile>,
    next_inode: u64,
    tick: u64,
}

/// In-memory `Vfs` keyed by `PathBuf`. Suitable for tests.
pub struct Mem {
    state: Mutex<MemState>,
    fail_next: Mutex<Option<(OpKind, io::Error)>>,
    /// WP1.S5: the counterpart to `fail_next`. `fail_next` intercepts a call
    /// before it touches `state`; `fail_after` lets a mutating op (currently
    /// `Exchange`/`RenameExcl`, the two publish primitives) complete its
    /// mutation and THEN fail, reproducing "the swap/rename already took
    /// effect, but the operation still reported failure" — the phase
    /// `WrappedIo::published` distinguishes, and which `fail_next` cannot
    /// express at all.
    fail_after: Mutex<Option<(OpKind, io::Error)>>,
}

impl Mem {
    pub fn new() -> Self {
        Mem {
            state: Mutex::new(MemState {
                files: HashMap::new(),
                next_inode: 1,
                tick: 0,
            }),
            fail_next: Mutex::new(None),
            fail_after: Mutex::new(None),
        }
    }

    /// Arms a one-shot failure for the next call to the `op` primitive. The
    /// failure fires exactly once (on the next matching call, regardless of
    /// how many non-matching calls happen first) and is then cleared.
    pub fn fail_next(&self, op: OpKind, kind: io::ErrorKind) {
        let err = io::Error::new(kind, format!("fail_next({op:?}) triggered"));
        let mut guard = self.fail_next.lock().unwrap_or_else(|p| p.into_inner());
        *guard = Some((op, err));
    }

    /// Arms a one-shot failure for the next `write_durable` — the first
    /// fallible step of `save_atomic`, so this reproduces the "next save
    /// fails" behavior a plain caller of `save_atomic` observes.
    pub fn fail_next_save(&self, kind: io::ErrorKind) {
        self.fail_next(OpKind::WriteDurable, kind);
    }

    /// Arms a one-shot failure for the next call to `op` that fires AFTER
    /// `op`'s mutation has already taken effect, marked
    /// [`crate::published_not_durable`] (only meaningful for `Exchange`/
    /// `RenameExcl`, the publish primitives `Disk::publish` also marks this
    /// way). See the field doc on `Mem::fail_after`.
    pub fn fail_after(&self, op: OpKind, kind: io::ErrorKind) {
        let err = io::Error::new(kind, format!("fail_after({op:?}) triggered"));
        let mut guard = self.fail_after.lock().unwrap_or_else(|p| p.into_inner());
        *guard = Some((op, err));
    }

    /// Consumes the armed failure if it targets `op`, returning it as an
    /// error. Otherwise leaves any differently-targeted armed failure
    /// untouched and returns `Ok`.
    fn take_failure(&self, op: OpKind) -> io::Result<()> {
        let mut guard = self.fail_next.lock().unwrap_or_else(|p| p.into_inner());
        match guard.as_ref() {
            Some((armed, _)) if *armed == op => {}
            _ => return Ok(()),
        }
        match guard.take() {
            Some((_, err)) => Err(err),
            None => Ok(()),
        }
    }

    /// Consumes the armed `fail_after` failure if it targets `op`, wrapping
    /// it as `published_not_durable` since every current caller of this
    /// (`exchange`/`rename_excl`) is a publish primitive.
    fn take_after_failure(&self, op: OpKind, context: impl Into<String>) -> Option<io::Error> {
        let mut guard = self.fail_after.lock().unwrap_or_else(|p| p.into_inner());
        match guard.as_ref() {
            Some((armed, _)) if *armed == op => {}
            _ => return None,
        }
        guard
            .take()
            .map(|(_, err)| crate::wrap_io_published(err, context))
    }

    fn lock_state(&self) -> MutexGuard<'_, MemState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Test/debug introspection only: every path currently stored,
    /// including orphaned temps a caller never published or removed —
    /// lets a test prove a temp file left behind by a failed publish still
    /// physically exists (§1.4.10's "capture/never silently discard"
    /// spirit) without hand-computing `temp_name`'s private naming scheme.
    pub fn debug_paths(&self) -> Vec<PathBuf> {
        self.lock_state().files.keys().cloned().collect()
    }

    /// Sets the hard-link count `Vfs::stat` reports for `path` (WP1.S6):
    /// lets a test drive the hardlink-fork data-safety warning path, which
    /// a hardcoded `nlink: Some(1)` made otherwise untestable against
    /// `Mem`. No-op (`Ok`) is not returned for a missing path — the caller
    /// gets `NotFound`, matching every other Mem primitive's shape.
    pub fn set_nlink(&self, path: &Path, nlink: u64) -> io::Result<()> {
        let mut state = self.lock_state();
        match state.files.get_mut(path) {
            Some(f) => {
                f.nlink = nlink;
                Ok(())
            }
            None => Err(not_found(path, "set_nlink")),
        }
    }
}

impl Default for Mem {
    fn default() -> Self {
        Self::new()
    }
}

/// See `Mem::resolve`. Anchors `path` at a synthetic root and collapses
/// `.`/`..` components against what came before, entirely lexically (no
/// filesystem access — `Mem` has none). A `..` past the root has nowhere to
/// go and is dropped, the same shape `Path::components()` already gives an
/// absolute path.
fn lexically_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.last(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            Component::RootDir => {
                out.clear();
                out.push(Component::RootDir);
            }
            Component::Normal(_) | Component::Prefix(_) => out.push(component),
        }
    }
    if !matches!(out.first(), Some(Component::RootDir)) {
        out.insert(0, Component::RootDir);
    }
    out.into_iter().collect()
}

/// `Mem` has no directory nodes (`MemState.files` is a flat
/// `HashMap<PathBuf, MemFile>`), so a directory exists at `path` iff some
/// stored key sits strictly below it — i.e. `key` starts with `path` plus at
/// least one more component.
fn sits_strictly_below(key: &Path, path: &Path) -> bool {
    key.strip_prefix(path)
        .map(|rest| rest.components().next().is_some())
        .unwrap_or(false)
}

fn not_found(path: &Path, op: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("{op} {}: not found in mem vfs", path.display()),
    )
}

impl Vfs for Mem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.take_failure(OpKind::Read)?;
        let state = self.lock_state();
        state
            .files
            .get(path)
            .map(|f| f.data.clone())
            .ok_or_else(|| not_found(path, "read"))
    }

    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        self.take_failure(OpKind::WriteDurable)?;
        let temp = temp_name(path);
        let mut state = self.lock_state();
        // Backend parity with `Disk::write_durable` (`OpenOptions::
        // create_new(true)`, which errors `AlreadyExists` rather than
        // silently truncating a colliding temp): a `HashMap::insert` here
        // would instead silently overwrite whatever the collision already
        // held, making that failure mode untestable against `Mem`.
        if state.files.contains_key(&temp) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("write_durable {}: temp already exists", temp.display()),
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
            },
        );
        Ok(temp)
    }

    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
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
        if let Some(e) = self.take_after_failure(
            OpKind::Exchange,
            format!("exchange {} <-> {}", a.display(), b.display()),
        ) {
            return Err(e);
        }
        Ok(())
    }

    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()> {
        self.take_failure(OpKind::RenameExcl)?;
        let mut state = self.lock_state();
        if !state.files.contains_key(old) {
            return Err(not_found(old, "renameexcl"));
        }
        if state.files.contains_key(new) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "renameexcl {} -> {}: destination exists",
                    old.display(),
                    new.display()
                ),
            ));
        }
        // Confirmed present above under this same (still-held) lock, so
        // this cannot miss.
        let Some(f) = state.files.remove(old) else {
            return Err(not_found(old, "renameexcl"));
        };
        state.files.insert(new.to_path_buf(), f);
        drop(state);
        if let Some(e) = self.take_after_failure(
            OpKind::RenameExcl,
            format!("renameexcl {} -> {}", old.display(), new.display()),
        ) {
            return Err(e);
        }
        Ok(())
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        self.take_failure(OpKind::Remove)?;
        let mut state = self.lock_state();
        if state.files.remove(path).is_none() {
            return Err(not_found(path, "remove"));
        }
        Ok(())
    }

    fn stat(&self, path: &Path) -> io::Result<Stat> {
        self.take_failure(OpKind::Stat)?;
        let state = self.lock_state();
        if let Some(f) = state.files.get(path) {
            return Ok(Stat {
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
                kind: FileKind::File,
            });
        }
        // No exact file at `path` — `Mem` has no directory nodes, so a
        // directory is synthesized: `path` is a directory iff some stored
        // key sits strictly below it.
        let is_synthetic_dir = state
            .files
            .keys()
            .any(|key| sits_strictly_below(key, path));
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
        self.take_failure(OpKind::Resolve)?;
        Ok(lexically_normalize(path))
    }

    /// No-op: Mem has no directory tree, only flat path->content keys.
    fn mkdir_all(&self, _path: &Path) -> io::Result<()> {
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
    /// state): the directory claim always wins, contributing `is_dir:
    /// true`, and the file claim for the same name is dropped rather than
    /// contributing a second, `is_dir: false` entry. Every key under `path`
    /// is folded into a `name -> is_dir` map first (dir overwrites file,
    /// never the reverse; a HashMap's iteration order can visit either key
    /// first) and only THEN turned into `entries`, so the result never
    /// depends on which of the colliding keys `state.files` happens to
    /// iterate first.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.take_failure(OpKind::ReadDir)?;
        let state = self.lock_state();
        // WP13.S1: the `PathBuf` travels alongside `is_dir` in the same
        // fold, built from `path.join(first)` — the byte-exact `Component`
        // straight off the stored key, never round-tripped through the
        // lossy `String` `name` also computed below.
        let mut by_name: HashMap<String, (bool, PathBuf)> = HashMap::new();
        // WP1.S6: `Disk::read_dir` on a nonexistent path errors `NotFound`;
        // `Mem` used to report an empty listing instead, since it derives
        // everything from key shape and a path with zero matching keys
        // looked identical to a genuinely empty (but existing) directory.
        // The synthetic root always exists; any other path needs either an
        // exact key (it's a stored file) or at least one key nested below
        // it (it's a synthetic directory) to count as present.
        let mut path_exists = path == Path::new("/") || state.files.contains_key(path);
        for key in state.files.keys() {
            let Ok(rest) = key.strip_prefix(path) else {
                continue;
            };
            let Some(first) = rest.components().next() else {
                // `rest` is empty: `key == path`, not a child of it.
                continue;
            };
            path_exists = true;
            let name = first.as_os_str().to_string_lossy().to_string();
            let child_path = path.join(first.as_os_str());
            // A key sits strictly below `child_path` itself: `first` is a
            // synthetic directory, not the file itself.
            let is_dir = sits_strictly_below(key, &child_path);
            let entry = by_name.entry(name).or_insert((is_dir, child_path));
            if is_dir {
                entry.0 = true;
            }
        }
        if !path_exists {
            return Err(not_found(path, "read_dir"));
        }
        let mut entries: Vec<DirEntry> = by_name
            .into_iter()
            .map(|(name, (is_dir, path))| DirEntry { name, path, is_dir })
            .collect();
        sort_dir_entries(&mut entries);
        Ok(entries)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// `stat` and `read_dir` derive "is this a synthetic directory" from the
    /// same predicate (`sits_strictly_below`); this exercises both entry
    /// points against the same fixture to prove they agree at every level —
    /// the file itself, its immediate parent, and an ancestor two levels up.
    #[test]
    fn stat_and_read_dir_agree_on_synthetic_directories() {
        let vfs = Mem::new();
        vfs.save_atomic(Path::new("/a/b/c.md"), b"content")
            .unwrap();

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
}
