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

use crate::{DirEntry, Identity, Stat, Vfs, sort_dir_entries, temp_name};

/// The `Vfs` operation a `Mem::fail_next` injection targets.
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

    fn lock_state(&self) -> MutexGuard<'_, MemState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Test/debug introspection only: every path currently stored,
    /// including orphaned temps a caller never published or removed. Used
    /// by `rune-db`'s materialize failure-path tests to prove a temp file
    /// left behind by a failed publish still physically exists (§1.4.10's
    /// "capture/never silently discard" spirit) without hand-computing
    /// `temp_name`'s private naming scheme.
    pub fn debug_paths(&self) -> Vec<PathBuf> {
        self.lock_state().files.keys().cloned().collect()
    }
}

impl Default for Mem {
    fn default() -> Self {
        Self::new()
    }
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
            },
        );
        Ok(temp)
    }

    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        self.take_failure(OpKind::Exchange)?;
        let mut state = self.lock_state();
        if !state.files.contains_key(a) {
            return Err(not_found(a, "exchange"));
        }
        if !state.files.contains_key(b) {
            return Err(not_found(b, "exchange"));
        }
        state.tick += 1;
        let mod_tick = state.tick;
        // Both keys were just confirmed present under the same lock, so
        // these removes cannot miss.
        if let (Some(mut fa), Some(mut fb)) = (state.files.remove(a), state.files.remove(b)) {
            fa.mod_tick = mod_tick;
            fb.mod_tick = mod_tick;
            state.files.insert(a.to_path_buf(), fb);
            state.files.insert(b.to_path_buf(), fa);
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
        if let Some(f) = state.files.remove(old) {
            state.files.insert(new.to_path_buf(), f);
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
                // Mem has no hardlink concept: every Mem file reports 1.
                nlink: Some(1),
                is_dir: false,
            });
        }
        // No exact file at `path` — `Mem` has no directory nodes (flat
        // `HashMap<PathBuf, MemFile>`), so a directory is synthesized the
        // same way `read_dir` derives one: `path` is a directory iff some
        // stored key sits strictly below it.
        let is_synthetic_dir = state.files.keys().any(|key| {
            key.strip_prefix(path)
                .map(|rest| rest.components().next().is_some())
                .unwrap_or(false)
        });
        if is_synthetic_dir {
            return Ok(Stat {
                size: 0,
                mtime: UNIX_EPOCH,
                identity: Identity::default(),
                nlink: None,
                is_dir: true,
            });
        }
        Err(not_found(path, "stat"))
    }

    /// Identity: Mem has no symlinks.
    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        self.take_failure(OpKind::Resolve)?;
        Ok(path.to_path_buf())
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
        let mut by_name: HashMap<String, bool> = HashMap::new();
        for key in state.files.keys() {
            let Ok(rest) = key.strip_prefix(path) else {
                continue;
            };
            let mut components = rest.components();
            let Some(first) = components.next() else {
                // `rest` is empty: `key == path`, not a child of it.
                continue;
            };
            let name = first.as_os_str().to_string_lossy().to_string();
            // More components remain below `first`: `first` is a synthetic
            // directory, not the file itself.
            let is_dir = components.next().is_some();
            let entry = by_name.entry(name).or_insert(is_dir);
            if is_dir {
                *entry = true;
            }
        }
        let mut entries: Vec<DirEntry> = by_name
            .into_iter()
            .map(|(name, is_dir)| DirEntry { name, is_dir })
            .collect();
        sort_dir_entries(&mut entries);
        Ok(entries)
    }
}
