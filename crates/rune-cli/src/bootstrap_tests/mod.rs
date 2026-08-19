//! Tests for `bootstrap`/`launch` — split out to keep the parent under the
//! file-size ceiling, the same shape `decode_cmd_tests.rs` (rune-tui)
//! already uses elsewhere in the workspace.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use rune_vfs::{Mem, VfsTestExt};
use std::sync::atomic::{AtomicU32, Ordering};

mod dead_session_recovery;
mod image_first;
mod launch_basics;
mod panic_and_diff;

/// Counts every [`Vfs::read`] and [`Vfs::resolve`] call made against ANY
/// path, wrapping a real [`Mem`] for everything else — the TOCTOU pin: a
/// launch's first positional must be read off disk exactly once (never once
/// for the buffer and again for the recovery store's own CAS baseline) AND
/// resolved exactly once (never once by `open_launch` and again inside
/// `rune_vfs::get`, which would reopen the symlink-swap TOCTOU window
/// between the two).
struct CountingReadVfs {
    inner: Mem,
    reads: AtomicU32,
    resolves: AtomicU32,
}

impl CountingReadVfs {
    fn new(inner: Mem) -> CountingReadVfs {
        CountingReadVfs {
            inner,
            reads: AtomicU32::new(0),
            resolves: AtomicU32::new(0),
        }
    }
}

impl Vfs for CountingReadVfs {
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.read(path)
    }
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
        self.inner.write_durable(path, bytes)
    }
    fn exchange(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        self.inner.exchange(a, b)
    }
    fn rename_excl(&self, old: &Path, new: &Path) -> std::io::Result<()> {
        self.inner.rename_excl(old, new)
    }
    fn remove(&self, path: &Path) -> std::io::Result<()> {
        self.inner.remove(path)
    }
    fn trash(&self, path: &Path) -> std::io::Result<()> {
        self.inner.trash(path)
    }
    fn stat(&self, path: &Path) -> std::io::Result<rune_vfs::Stat> {
        self.inner.stat(path)
    }
    fn resolve(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.resolves.fetch_add(1, Ordering::SeqCst);
        self.inner.resolve(path)
    }
    fn mkdir_all(&self, path: &Path) -> std::io::Result<()> {
        self.inner.mkdir_all(path)
    }
    fn read_dir(&self, path: &Path) -> std::io::Result<Vec<rune_vfs::DirEntry>> {
        self.inner.read_dir(path)
    }
    fn read_link(&self, path: &Path) -> std::io::Result<PathBuf> {
        self.inner.read_link(path)
    }
}

struct ScratchHome(PathBuf);

impl ScratchHome {
    fn new(label: &str) -> ScratchHome {
        let dir = env::temp_dir().join(format!(
            "rune-cli-launch-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create scratch home");
        ScratchHome(dir)
    }
}

impl Drop for ScratchHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
