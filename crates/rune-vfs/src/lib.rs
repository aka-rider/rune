//! `rune-vfs`: the single chokepoint for real-disk I/O of the user's `.md`
//! documents.
//!
//! The trait is the **materialize-complete primitive set**: `read`,
//! `write_durable`, `exchange`, `rename_excl`, `remove`, `stat`, `resolve`,
//! `mkdir_all`. The shape is already split so a caller can capture
//! displaced bytes before they're discarded — see the module docs on
//! `Vfs::save_atomic` below.
//!
//! Two implementations: `Disk` (production, Darwin `renamex_np` atomic
//! publish) and `Mem` (fully in-memory, for tests and — eventually — the
//! session fuzzer).

mod disk;
mod etag;
mod mem;
mod publish;
mod sighting;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;
use std::{io, process};

pub use disk::Disk;
pub use etag::{Etag, etag_of};
pub use mem::{Mem, OpKind};
pub use publish::{PutCondition, PutOutcome, put};
pub use sighting::{GetRefusal, MAX_DOCUMENT_BYTES, Sighting, get};

/// Error-wrap chokepoint (WP1.S4): wraps `e` with `context` while keeping
/// `e` itself reachable as [`std::error::Error::source`] — so a caller can
/// still classify the ORIGINAL failure (`kind()`, `raw_os_error()`) instead
/// of that classification being erased into a display string, which is what
/// the naive `io::Error::new(e.kind(), format!(...))` pattern this replaces
/// used to do (`raw_os_error()` is only ever `Some` on an unwrapped OS
/// error).
#[derive(Debug)]
struct WrappedIo {
    context: String,
    source: io::Error,
    /// WP1.S1: set only when the underlying swap/rename already took effect
    /// before this error occurred (a post-publish durability confirmation
    /// failure, e.g. `Disk`'s parent fsync or a `Mem::fail_after`
    /// injection) — see [`published_not_durable`].
    published: bool,
}

impl std::fmt::Display for WrappedIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.context, self.source)
    }
}

impl std::error::Error for WrappedIo {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// See [`WrappedIo`]. Preserves `e`'s `kind()` on the outer `io::Error`.
pub(crate) fn wrap_io(e: io::Error, context: impl Into<String>) -> io::Error {
    let kind = e.kind();
    io::Error::new(
        kind,
        WrappedIo {
            context: context.into(),
            source: e,
            published: false,
        },
    )
}

/// Same as [`wrap_io`], additionally marking the error as
/// [`published_not_durable`]: the publish (swap/rename) already took effect
/// when this failure occurred.
pub(crate) fn wrap_io_published(e: io::Error, context: impl Into<String>) -> io::Error {
    let kind = e.kind();
    io::Error::new(
        kind,
        WrappedIo {
            context: context.into(),
            source: e,
            published: true,
        },
    )
}

/// True when `e` carries the [`wrap_io_published`] marker: an
/// `exchange`/`rename_excl` publish already took effect before `e`
/// occurred, so a temp file naming the operation's source path holds
/// content (the sole surviving copy of whatever the publish displaced, or
/// of the caller's own just-published bytes) that must not be discarded —
/// `save_atomic` is the first caller (WP1.S1/S2).
pub fn published_not_durable(e: &io::Error) -> bool {
    e.get_ref()
        .and_then(|inner| inner.downcast_ref::<WrappedIo>())
        .is_some_and(|w| w.published)
}

/// The stable (inode, device) identity of a file. History is keyed to it
/// rather than the path so a rename does not orphan history. `None` fields
/// mean the platform or backend doesn't expose that half of identity —
/// always SQL-NULL-shaped `Option`, never a sentinel `0` (a sentinel value
/// would be indistinguishable from a real identity that happens to be zero).
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
/// after the atomic swap. `Identity` already derives them.
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
    /// What kind of filesystem object the path names. Link resolution accepts
    /// only `File`: a directory cannot be opened as a buffer, and reading a
    /// FIFO, socket or device node would block the caller forever.
    pub kind: FileKind,
}

/// The kind of object a path names. Deliberately an enum rather than a pair
/// of booleans so "neither a file nor a directory" is a representable, named
/// state instead of an accidental one — the distinction matters because
/// `Vfs::read` on a FIFO or socket never returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    File,
    Dir,
    Other,
}

/// A single direct child of a directory, as returned by [`Vfs::read_dir`].
///
/// `name` and `path` are DELIBERATELY both carried, additively (plan
/// WP13.S1 — this is not a "pick one" API): `name` is the lossy-decoded
/// `String` display/sort form (`sort_dir_entries`'s `to_lowercase`, and
/// `rune-fuzz`'s text script codec, both need a total, always-valid `str`
/// that an `OsString` cannot give them without breaking script replay);
/// `path` is the byte-exact full path to open — never lossy-decoded and
/// never rebuilt by joining `name` back onto a parent — joining a
/// lossy-decoded name into a real path is what let the app mangle and then
/// open a name the user never had. A caller that needs to OPEN the entry
/// uses `path`; a caller that only displays or sorts uses `name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// The entry's own name (not a full path) — lossy-decoded, display and
    /// sort only. Never join this back onto a directory to build an
    /// openable path; use `path` instead.
    pub name: String,
    /// The entry's full, byte-exact path — what `Disk` read from the raw
    /// `file_name()` `OsString` (never round-tripped through `String`) and
    /// what `Mem` derived from its own key. Always safe to open.
    pub path: PathBuf,
    pub is_dir: bool,
}

/// A virtual file system exposing the materialize-complete primitive set,
/// plus `read_dir` for directory enumeration. `Rename` is still deferred to
/// the feature that consumes it.
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
    /// publish displaces — capturing displaced bytes as a durable blob
    /// before they're ever discarded is the property `save_atomic` alone
    /// cannot express, because it destroys the displaced bytes internally
    /// (see its docs).
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf>;

    /// Atomically swap the contents of `a` and `b` (both must already
    /// exist; same volume). Disk: `renamex_np(RENAME_SWAP)` + parent fsync
    /// — a single kernel operation, so neither path is ever unlinked; that's
    /// the durability guarantee for the publish. Mem: swaps the two
    /// entries' file objects between keys (inodes travel with content).
    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()>;

    /// Atomically rename `old` to `new`, failing with an error wrapping
    /// `io::ErrorKind::AlreadyExists` if `new` already exists — no clobber:
    /// atomic publish via `RenameExcl` leaves no window for a silent
    /// overwrite. Disk: `renamex_np(RENAME_EXCL)` + parent fsync.
    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()>;

    /// Delete a single file. Not `Trash` — internal temps must never shell
    /// out to `/usr/bin/trash` (that's a user-facing operation).
    fn remove(&self, path: &Path) -> io::Result<()>;

    /// Move a single file to the OS Trash — the user-facing, recoverable
    /// counterpart of `remove`, which stays reserved for internal temp
    /// cleanup.
    fn trash(&self, path: &Path) -> io::Result<()>;

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
    /// satisfy capture-before-discard on its own; a caller that
    /// needs the displaced content must call `write_durable`/`exchange`
    /// directly instead (a later work package wires `materialize` that
    /// way). Kept only so existing callers (the plain `super+s` save path)
    /// keep working unchanged through this work package.
    fn save_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let outcome = publish::put(self, path, bytes, PutCondition::Force { expect: None })?;
        match outcome {
            PutOutcome::Committed { durable: true, .. }
            | PutOutcome::Raced { durable: true, .. } => Ok(()),
            PutOutcome::Committed { durable: false, .. }
            | PutOutcome::Raced { durable: false, .. } => {
                // The publish already took effect but its durability could
                // not be confirmed — the sibling temp holding the displaced
                // content stays on disk rather than being removed; the
                // marker is carried onto the re-wrapped error so the
                // condition remains observable further up.
                Err(wrap_io_published(
                    io::Error::other("durability could not be confirmed after publish"),
                    "save published but durability could not be confirmed; \
                     the prior content is preserved on a sibling temp file",
                ))
            }
            PutOutcome::Missing | PutOutcome::Conflict { .. } => Err(io::Error::other(
                "save_atomic: an unconditional Force publish reported Missing or Conflict",
            )),
        }
    }
}

/// The single chokepoint for `read_dir`'s ordering contract — shared by
/// `Disk` and `Mem` so both backends sort identically instead of each
/// re-implementing the comparator. Directories sort before files; within
/// each group, the primary key is the LOWERCASED name, with the exact
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
/// `.{basename}.rune-tmp-{pid}-{n}`, in `path`'s own parent directory (so
/// the eventual `exchange`/`rename_excl` publish is same-volume). `n` comes
/// from a process-wide counter so two temps for the same `path` in the same
/// process never collide even when a prior one was never cleaned up (a
/// pid-only name would otherwise wedge every later save of that path on a
/// leftover crash residue). Shared by `Disk` and `Mem` so both backends
/// produce the same temp-name shape.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) fn temp_name(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or(Path::new("")).to_path_buf();
    let basename = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let pid = process::id();
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".{basename}.rune-tmp-{pid}-{n}"))
}
