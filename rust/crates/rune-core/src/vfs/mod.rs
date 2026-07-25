//! Virtual file system abstraction with Darwin-specific atomic save semantics.
//!
//! This module provides a `Vfs` trait with two concrete implementations:
//!
//! - **`Disk`** — A real filesystem backend on macOS that guarantees
//!   atomic, crash-safe writes via the `renamex_np` syscall. The save
//!   algorithm follows CONSTITUTION §1.4.1 (port of Go's `Exchange`/
//!   `RenameExcl`):
//!   1. Write bytes to a temp file `.<name>.rune-tmp-<pid>` in the same
//!      directory as the destination.
//!   2. `fsync` the temp file.
//!   3. If the destination already exists, atomically swap it with the
//!      temp file via `renamex_np(..., RENAME_SWAP)`, then remove the
//!      old destination.
//!   4. If the destination does not exist, create it atomically via
//!      `renamex_np(..., RENAME_EXCL)`.
//!   5. `fsync` the parent directory to make the rename durable.
//!   6. On any error, clean up the temp file and propagate the error.
//!
//! - **`Mem`** — An in-memory key-value store keyed by `Path`-derived
//!   strings, suitable for tests. Mirrors the semantics of Go's
//!   `pkg/vfs/mem.go` (a `sync.Map`-backed `map[string][]byte`).
//!
//! Both implementations expose `&self`-receiver methods (interior
//! mutability in `Mem` via `Mutex`), so the trait works behind shared
//! references.
//!
//! # Design rationale
//!
//! The `Vfs` trait isolates persistence from the editor core. `Disk`
//! gives production-grade atomicity on Darwin; `Mem` lets tests verify
//! editor logic without touching the real filesystem. A future `Sqlite`
//! backend would implement the same trait (§1.4.10, "durable store").

mod disk;
mod mem;

use std::io;
use std::path::Path;

/// A virtual file system with read and atomic-save operations.
///
/// Both methods take `&self` (not `&mut self`) so implementations can
/// use interior mutability — `Disk` is `!Sync`-free (stateless), `Mem`
/// uses `Mutex`.
pub trait Vfs {
    /// Read the entire contents of `path`.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;

    /// Atomically save `bytes` to `path`.
    ///
    /// On `Disk`, this uses the `renamex_np` algorithm described in the
    /// module docs. On `Mem`, this replaces any existing value for the
    /// key.
    fn save_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
}

pub use disk::Disk;
pub use mem::Mem;
