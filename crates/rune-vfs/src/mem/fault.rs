use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::FileKind;
use crate::path_util::{lexically_normalize, not_found};

use super::{Mem, MemFile};

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

pub(crate) struct Faults {
    pub fail_next: Mutex<Option<(OpKind, io::Error)>>,
    pub fail_after: Mutex<Option<(OpKind, io::Error)>>,
    pub mutate_after_stat: Mutex<Option<(PathBuf, Vec<u8>)>>,
    pub churning: Mutex<HashSet<PathBuf>>,
    pub resolve_failures: Mutex<HashSet<PathBuf>>,
}

impl Faults {
    pub(crate) fn new() -> Self {
        Faults {
            fail_next: Mutex::new(None),
            fail_after: Mutex::new(None),
            mutate_after_stat: Mutex::new(None),
            churning: Mutex::new(HashSet::new()),
            resolve_failures: Mutex::new(HashSet::new()),
        }
    }
}

impl Mem {
    /// Arms a one-shot failure for the next call to the `op` primitive. The
    /// failure fires exactly once (on the next matching call, regardless of
    /// how many non-matching calls happen first) and is then cleared.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn fail_next(&self, op: OpKind, kind: io::ErrorKind) {
        let err = io::Error::new(kind, format!("fail_next({op:?}) triggered"));
        let mut guard = self
            .faults
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
            .faults
            .fail_after
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some((op, err));
    }

    /// Consumes the armed failure if it targets `op`, returning it as an
    /// error. Otherwise leaves any differently-targeted armed failure
    /// untouched and returns `Ok`.
    #[cfg(any(test, feature = "fault-injection"))]
    pub(super) fn take_failure(&self, op: OpKind) -> io::Result<()> {
        let mut guard = self
            .faults
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
    pub(super) fn take_after_failure(
        &self,
        op: OpKind,
        context: impl Into<String>,
    ) -> Option<io::Error> {
        let mut guard = self
            .faults
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
            .faults
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
            .faults
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
    pub(super) fn apply_pending_mutation(&self, path: &Path) {
        let is_churning = self
            .faults
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
                .faults
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
            .faults
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
