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

/// The `Vfs` operation a `Mem::fail_next`/`Mem::fail_after` injection
/// targets.
#[cfg(any(test, feature = "fault-injection"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpKind {
    Read,
    WriteDurable,
    Exchange,
    RenameExcl,
    Remove,
    Trash,
    Stat,
    Resolve,
    MkdirAll,
    ReadDir,
    ReadLink,
}

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
    fail_next: Mutex<Option<(OpKind, io::Error)>>,
    /// WP1.S5: the counterpart to `fail_next`. `fail_next` intercepts a call
    /// before it touches `state`; `fail_after` lets a mutating op (currently
    /// `Exchange`/`RenameExcl`, the two publish primitives) complete its
    /// mutation and THEN fail, reproducing "the swap/rename already took
    /// effect, but the operation still reported failure" — the phase
    /// `WrappedIo::published` distinguishes, and which `fail_next` cannot
    /// express at all.
    #[cfg(any(test, feature = "fault-injection"))]
    fail_after: Mutex<Option<(OpKind, io::Error)>>,
    /// WP-A: a one-shot mutation that fires the NEXT time `Vfs::stat(path)`
    /// is called, applied AFTER that call has already computed its answer —
    /// reproducing "the file changed in the gap between two stat calls that
    /// bracket a read": the bracket's first stat sees the state as it was,
    /// its second stat (or the read in between) sees the state after.
    #[cfg(any(test, feature = "fault-injection"))]
    mutate_after_stat: Mutex<Option<(PathBuf, Vec<u8>)>>,
    /// WP-A: paths currently in "churn" mode — EVERY `Vfs::stat` call
    /// mutates content+identity right after computing its answer, forever,
    /// rather than the one-shot `mutate_after_stat`. Reproduces a file that
    /// never stops changing: no bracket around it can ever settle, since
    /// even its own retry attempts each see a fresh mutation mid-bracket.
    #[cfg(any(test, feature = "fault-injection"))]
    churning: Mutex<std::collections::HashSet<PathBuf>>,
    /// Paths `Mem::resolve` refuses permanently, set via `Mem::fail_resolve`
    /// — an unreadable/missing ancestor or a symlink loop, unlike
    /// `fail_next(OpKind::Resolve, ..)`'s one-shot, path-blind trigger,
    /// which cannot target one path across a test that resolves several.
    #[cfg(any(test, feature = "fault-injection"))]
    resolve_failures: Mutex<std::collections::HashSet<PathBuf>>,
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
            fail_next: Mutex::new(None),
            #[cfg(any(test, feature = "fault-injection"))]
            fail_after: Mutex::new(None),
            #[cfg(any(test, feature = "fault-injection"))]
            mutate_after_stat: Mutex::new(None),
            #[cfg(any(test, feature = "fault-injection"))]
            churning: Mutex::new(std::collections::HashSet::new()),
            #[cfg(any(test, feature = "fault-injection"))]
            resolve_failures: Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Arms a one-shot failure for the next call to the `op` primitive. The
    /// failure fires exactly once (on the next matching call, regardless of
    /// how many non-matching calls happen first) and is then cleared.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn fail_next(&self, op: OpKind, kind: io::ErrorKind) {
        let err = io::Error::new(kind, format!("fail_next({op:?}) triggered"));
        let mut guard = self
            .fail_next
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some((op, err));
    }

    /// Arms a one-shot failure for the next `write_durable` — the first
    /// fallible step of `save_atomic`, so this reproduces the "next save
    /// fails" behavior a plain caller of `save_atomic` observes.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn fail_next_save(&self, kind: io::ErrorKind) {
        self.fail_next(OpKind::WriteDurable, kind);
    }

    /// Arms a one-shot failure for the next call to `op` that fires AFTER
    /// `op`'s mutation has already taken effect, marked
    /// [`crate::published_not_durable`] (only meaningful for `Exchange`/
    /// `RenameExcl`, the publish primitives `Disk::publish` also marks this
    /// way). See the field doc on `Mem::fail_after`.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn fail_after(&self, op: OpKind, kind: io::ErrorKind) {
        let err = io::Error::new(kind, format!("fail_after({op:?}) triggered"));
        let mut guard = self
            .fail_after
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some((op, err));
    }

    /// Consumes the armed failure if it targets `op`, returning it as an
    /// error. Otherwise leaves any differently-targeted armed failure
    /// untouched and returns `Ok`.
    #[cfg(any(test, feature = "fault-injection"))]
    fn take_failure(&self, op: OpKind) -> io::Result<()> {
        let mut guard = self
            .fail_next
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    #[cfg(any(test, feature = "fault-injection"))]
    fn take_after_failure(&self, op: OpKind, context: impl Into<String>) -> Option<io::Error> {
        let mut guard = self
            .fail_after
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.as_ref() {
            Some((armed, _)) if *armed == op => {}
            _ => return None,
        }
        guard
            .take()
            .map(|(_, err)| crate::wrap_io_published(err, context))
    }

    fn lock_state(&self) -> MutexGuard<'_, MemState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Test/debug introspection only: every path currently stored,
    /// including orphaned temps a caller never published or removed —
    /// lets a test prove a temp file left behind by a failed publish still
    /// physically exists (nothing is ever silently discarded) without
    /// hand-computing `temp_name`'s private naming scheme.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn debug_paths(&self) -> Vec<PathBuf> {
        self.lock_state().files.keys().cloned().collect()
    }

    /// Test/fault-injection hook (WP-A): overwrites `path`'s content in
    /// place WITHOUT touching its `inode`/`device`/`mod_tick` — the
    /// same-tick/same-identity external rewrite a stat-only "nothing
    /// changed" comparison cannot detect by construction, since two
    /// different writes land on identical stat facts. Errors `NotFound` if
    /// `path` doesn't already exist, matching every other `Mem` primitive's
    /// shape.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn set_content_keep_identity(&self, path: &Path, bytes: Vec<u8>) -> io::Result<()> {
        let mut state = self.lock_state();
        match state.files.get_mut(path) {
            Some(f) => {
                f.data = bytes;
                Ok(())
            }
            None => Err(not_found(path, "set_content_keep_identity")),
        }
    }

    /// Arms a one-shot mutation that fires the NEXT time `Vfs::stat(path)`
    /// is called: that call still answers with `path`'s CURRENT state, but
    /// immediately afterward `path`'s content is replaced and its identity
    /// (inode, mod_tick) minted fresh — reproducing "the file changed in the
    /// gap between two stat calls that bracket a read", the mid-bracket
    /// mutation a stat-read-stat confirmation must catch by re-reading
    /// rather than trusting a single stat pair.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn mutate_after_next_stat(&self, path: &Path, bytes: Vec<u8>) {
        let mut guard = self
            .mutate_after_stat
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some((path.to_path_buf(), bytes));
    }

    /// Puts `path` into (or takes it out of) perpetual "churn" mode: while
    /// churning, EVERY `Vfs::stat(path)` call mutates content+identity right
    /// after computing its answer, forever — a file that never stops
    /// changing, so no bracket around it (including its own retry
    /// attempts) can ever settle. Represents a disk that keeps disagreeing
    /// with itself across every re-probe, the shape
    /// [`crate::mem`]'s bracket-retry ceiling must degrade to unconfirmed
    /// against.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn set_churning(&self, path: &Path, churning: bool) {
        let mut guard = self
            .churning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if churning {
            guard.insert(path.to_path_buf());
        } else {
            guard.remove(path);
        }
    }

    /// Mutates `path`'s content to a fresh, unique payload and mints a
    /// fresh identity — the shared body behind churn mode and the one-shot
    /// [`Mem::mutate_after_next_stat`] hook.
    #[cfg(any(test, feature = "fault-injection"))]
    fn mutate_now(&self, path: &Path, bytes: Vec<u8>) {
        let mut state = self.lock_state();
        state.tick += 1;
        let mod_tick = state.tick;
        let inode = state.next_inode;
        state.next_inode += 1;
        if let Some(f) = state.files.get_mut(path) {
            f.data = bytes;
            f.inode = inode;
            f.mod_tick = mod_tick;
        }
    }

    /// Applies whichever pending mutation targets `path` (churn mode takes
    /// priority, since it fires unconditionally; the one-shot hook is
    /// consumed at most once) — called from `Vfs::stat` after it has
    /// already read the answer it is about to return.
    #[cfg(any(test, feature = "fault-injection"))]
    fn apply_pending_mutation(&self, path: &Path) {
        let is_churning = self
            .churning
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(path);
        if is_churning {
            let tick = self.lock_state().tick;
            self.mutate_now(path, format!("churn {tick}").into_bytes());
            return;
        }
        let armed = {
            let mut guard = self
                .mutate_after_stat
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match guard.as_ref() {
                Some((armed_path, _)) if armed_path == path => guard.take(),
                _ => None,
            }
        };
        let Some((_, bytes)) = armed else { return };
        self.mutate_now(path, bytes);
    }

    /// Sets the hard-link count `Vfs::stat` reports for `path` (WP1.S6):
    /// lets a test drive the hardlink-fork data-safety warning path, which
    /// a hardcoded `nlink: Some(1)` made otherwise untestable against
    /// `Mem`. No-op (`Ok`) is not returned for a missing path — the caller
    /// gets `NotFound`, matching every other Mem primitive's shape.
    #[cfg(any(test, feature = "fault-injection"))]
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

    /// Sets the `FileKind` `Vfs::stat` reports for `path`: lets a
    /// test model a FIFO, socket, or device node — `Mem` otherwise only
    /// ever represents `FileKind::File`/`Dir`. No-op (`Ok`) is not returned
    /// for a missing path — the caller gets `NotFound`, matching every
    /// other `Mem` primitive's shape.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn set_kind(&self, path: &Path, kind: FileKind) -> io::Result<()> {
        let mut state = self.lock_state();
        match state.files.get_mut(path) {
            Some(f) => {
                f.kind = kind;
                Ok(())
            }
            None => Err(not_found(path, "set_kind")),
        }
    }

    /// Marks `path` (any spelling, compared after `lexically_normalize`) so
    /// every future `Vfs::resolve` call naming it fails permanently — a
    /// path-targeted counterpart to `fail_next(OpKind::Resolve, ..)`'s
    /// one-shot, path-blind trigger, for a test that needs one specific
    /// path's resolution to be the one that fails while others still
    /// succeed. No existing-file requirement, unlike `set_nlink`/`set_kind`
    /// — resolution failure (a missing/unreadable ancestor, a symlink loop)
    /// is exactly the case where the target need not exist at all.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn fail_resolve(&self, path: &Path) {
        let mut guard = self
            .resolve_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.insert(lexically_normalize(path));
    }

    /// Seeds a symlink at `link` naming `target`, which need not exist —
    /// a dangling link is exactly what `Link::Broken` must be driven by.
    /// `target` may be relative, in which case it resolves against `link`'s
    /// parent, matching `std::os::unix::fs::symlink`.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn symlink(&self, link: &Path, target: &Path) -> io::Result<()> {
        let link = lexically_normalize(link);
        let mut state = self.lock_state();
        if state.files.contains_key(&link) {
            return Err(crate::wrap_io(
                io::Error::new(io::ErrorKind::AlreadyExists, "path already exists"),
                format!("symlink {}", link.display()),
            ));
        }
        state.tick += 1;
        let mod_tick = state.tick;
        let inode = state.next_inode;
        state.next_inode += 1;
        state.files.insert(
            link,
            MemFile {
                data: Vec::new(),
                inode,
                device: 1,
                mod_tick,
                nlink: 1,
                kind: FileKind::Other,
                link_target: Some(target.to_path_buf()),
            },
        );
        Ok(())
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
