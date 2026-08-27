#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use rune_vfs::{Mem, VfsTestExt};
use std::sync::atomic::{AtomicU32, Ordering};

mod dead_session_recovery;
mod image_first;
mod launch_basics;
mod panic_and_diff;

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

pub(crate) struct ScratchHome(pub(crate) PathBuf);

impl ScratchHome {
    pub(crate) fn new(label: &str) -> ScratchHome {
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
