//! `rune-vfs`: the single chokepoint for real-disk I/O of the user's `.md`
//! documents (CONSTITUTION §1.4.9). Ported from Go's vfs package.
//!
//! The trait is the **materialize-complete primitive set**: `read`,
//! `write_durable`, `exchange`, `rename_excl`, `remove`, `stat`, `resolve`,
//! `mkdir_all`. Unlike the Go interface (which is method-parity with
//! `os.*`), the Rust shape is already split so a caller can capture
//! displaced bytes before they're discarded (§1.4.10) — see the module docs
//! on `Vfs::save_atomic` and the `capture_before_discard` test.
//!
//! Two implementations: `Disk` (production, Darwin `renamex_np` atomic
//! publish) and `Mem` (fully in-memory, for tests and — eventually — the
//! session fuzzer), mirroring Go's own `Disk` / `Mem` pair.

mod disk;
mod mem;

use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::{io, process};

pub use disk::Disk;
pub use mem::{Mem, OpKind};

/// The stable (inode, device) identity of a file. History is keyed to it
/// rather than the path so a rename does not orphan history (CONSTITUTION
/// §1.4.6). `None` fields mean the platform or backend doesn't expose that
/// half of identity — always SQL-NULL-shaped `Option`, never a sentinel `0`
/// (the Go schema went through v8/v9 specifically to undo that mistake).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub inode: Option<u64>,
    pub device: Option<u64>,
}

/// File metadata returned by `Vfs::stat`.
///
/// `PartialEq`/`Eq` exist for the rename-replace **consent** check: the
/// `[R]eplace` guard records the `Stat` the user was shown, and the replace
/// op re-stats the destination and refuses when it no longer matches ("still
/// the file you agreed to replace?"). That comparison is a consent check, not
/// the safety mechanism — safety comes from capturing the displaced bytes
/// after the atomic swap (§1.4.10). `Identity` already derives them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub size: u64,
    pub mtime: SystemTime,
    pub identity: Identity,
    /// Hard-link count. `None` when the platform/backend doesn't expose it.
    /// A value greater than 1 means a save through this path would silently
    /// fork the document from its other names on disk (materialize surfaces
    /// this as a footer warning — a later work package).
    pub nlink: Option<u64>,
}

/// A single direct child of a directory, as returned by [`Vfs::read_dir`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// The entry's own name (not a full path) — the final component you'd
    /// join onto the directory that was listed.
    pub name: String,
    pub is_dir: bool,
}

/// A virtual file system exposing the materialize-complete primitive set
/// (plan decision 12), plus `read_dir` for directory enumeration.
/// `Trash`/`Rename` are still deferred to the features that consume them.
///
/// All methods take `&self` (not `&mut self`) so implementations can use
/// interior mutability — `Disk` is stateless, `Mem` uses `Mutex`.
pub trait Vfs {
    /// Read the entire contents of `path`.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Durably write `bytes` to a fresh sibling temp file next to `path` —
    /// fsync'd, but **not renamed onto `path` and not unlinked**. Returns
    /// the temp file's path so the caller can publish it (`exchange` /
    /// `rename_excl`) and, critically, can still `read` whatever the
    /// publish displaces (§1.4.10 "capture displaced bytes as a durable
    /// blob before they're ever discarded") — the property `save_atomic`
    /// alone cannot express, because it destroys the displaced bytes
    /// internally (see its docs).
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf>;

    /// Atomically swap the contents of `a` and `b` (both must already
    /// exist; same volume). Disk: `renamex_np(RENAME_SWAP)` + parent fsync
    /// — a single kernel operation, so neither path is ever unlinked; the
    /// durability guarantee for the publish (§1.4.1). Mem: swaps the two
    /// entries' file objects between keys (inodes travel with content).
    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()>;

    /// Atomically rename `old` to `new`, failing with an error wrapping
    /// `io::ErrorKind::AlreadyExists` if `new` already exists — no clobber
    /// (§1.4.1: atomic publish via `RenameExcl`, no window for a silent
    /// overwrite). Disk: `renamex_np(RENAME_EXCL)` + parent fsync.
    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()>;

    /// Delete a single file. Not `Trash` — internal temps must never shell
    /// out to `/usr/bin/trash` (that's a user-facing operation).
    fn remove(&self, path: &Path) -> io::Result<()>;

    /// Stat `path`, returning size/mtime/identity/nlink.
    fn stat(&self, path: &Path) -> io::Result<Stat>;

    /// Canonicalize `path`. Disk: resolves symlinks so saves write through a
    /// symlink to its target, never over the link itself; when the leaf
    /// doesn't exist yet (first save of a new file), only the existing
    /// parent is resolved and the unresolved leaf name is re-joined. Mem:
    /// identity (no symlinks).
    fn resolve(&self, path: &Path) -> io::Result<PathBuf>;

    /// Create `path` and all missing parent directories.
    fn mkdir_all(&self, path: &Path) -> io::Result<()>;

    /// List the direct children of `path` (not recursive). Order is part of
    /// the contract, not an implementation accident: directories first, then
    /// files, and case-sensitive by `name` within each group — callers (e.g.
    /// a filetree) can rely on this instead of re-sorting.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;

    /// Atomically save `bytes` to `path`, composed from the primitives
    /// above: `resolve` the destination, `write_durable` the bytes to a
    /// sibling temp, then publish via `exchange` (destination exists) or
    /// `rename_excl` (destination is new).
    ///
    /// This is a **compatibility convenience** for callers that don't need
    /// the displaced bytes — it deletes the temp (and, on the SWAP path,
    /// the bytes the swap just displaced) as its last step, so it CANNOT
    /// satisfy §1.4.10's capture-before-discard on its own; a caller that
    /// needs the displaced content must call `write_durable`/`exchange`
    /// directly instead (a later work package wires `materialize` that
    /// way). Kept only so existing callers (the plain `super+s` save path)
    /// keep working unchanged through this work package.
    fn save_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let dest = self.resolve(path)?;
        let dest_existed = self.stat(&dest).is_ok();
        let temp = self.write_durable(&dest, bytes)?;

        let publish = if dest_existed {
            self.exchange(&temp, &dest)
        } else {
            self.rename_excl(&temp, &dest)
        };

        match publish {
            Ok(()) => {
                if dest_existed {
                    // The swap displaced the old content onto `temp`; this
                    // convenience has no caller to hand it to, so it's
                    // discarded here — see the doc comment above.
                    let _ = self.remove(&temp);
                }
                Ok(())
            }
            Err(e) => {
                let _ = self.remove(&temp);
                Err(e)
            }
        }
    }
}

/// The single chokepoint for `read_dir`'s ordering contract — shared by
/// `Disk` and `Mem` so both backends sort identically instead of each
/// re-implementing the comparator. Directories sort before files; within
/// each group, the primary key is the LOWERCASED name (matching the Go
/// reference's `strings.ToLower(a.Name) < strings.ToLower(b.Name)`), with the exact
/// (original-case) name as a tie-break so two names differing only by case
/// (`"File.md"` vs `"file.md"`) still sort deterministically rather than
/// depending on `sort_by`'s stability + whatever order the caller happened
/// to hand them in.
pub(crate) fn sort_dir_entries(entries: &mut [DirEntry]) {
    entries.sort_by(|a, b| {
        // `is_dir: true` (dirs) must sort before `false` (files): reverse
        // the natural bool order, then break ties case-insensitively, then
        // by exact name for determinism.
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// The sibling temp filename a durable write uses for `path`:
/// `.{basename}.rune-tmp-{pid}`, in `path`'s own parent directory (so the
/// eventual `exchange`/`rename_excl` publish is same-volume). Shared by
/// `Disk` and `Mem` so both backends produce the same temp-name shape.
pub(crate) fn temp_name(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("")).to_path_buf();
    let basename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let pid = process::id();
    parent.join(format!(".{basename}.rune-tmp-{pid}"))
}
