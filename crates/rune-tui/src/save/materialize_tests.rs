//! Unit tests for `save/materialize.rs`, kept in a sibling file so that
//! module itself stays inside the 500-line budget.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rune_vfs::{DirEntry, Mem, Stat, Vfs};

use super::*;

/// Wraps a `Mem`, delegating every `Vfs` method to it verbatim EXCEPT
/// `read`, which returns `swap_at`'s bytes for `path`'s Nth call (1-indexed)
/// and the inner `Mem`'s own content otherwise — lets a test drive exactly
/// what the CAS re-verify loop sees on its first vs. second read of the live
/// target without needing to interleave code between library calls.
struct SwappingReadVfs {
    inner: Mem,
    path: PathBuf,
    swap_at: usize,
    swap_to: Vec<u8>,
    read_calls: AtomicUsize,
}

impl Vfs for SwappingReadVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        if path == self.path {
            let call = self.read_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.swap_at {
                return Ok(self.swap_to.clone());
            }
        }
        self.inner.read(path)
    }
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        self.inner.write_durable(path, bytes)
    }
    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        self.inner.exchange(a, b)
    }
    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()> {
        self.inner.rename_excl(old, new)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.inner.remove(path)
    }
    fn trash(&self, path: &Path) -> io::Result<()> {
        self.inner.trash(path)
    }
    fn stat(&self, path: &Path) -> io::Result<Stat> {
        self.inner.stat(path)
    }
    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.resolve(path)
    }
    fn mkdir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.mkdir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.inner.read_dir(path)
    }
}

fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

/// Task WP-A(4): a CAS mismatch on the FIRST read that stops reproducing on
/// the SECOND (a transient external window that closed before the save
/// itself proceeds) must not raise a conflict — the save proceeds and
/// commits against the now-matching live content.
#[test]
fn a_transient_cas_mismatch_that_closes_on_the_second_read_proceeds_to_commit() {
    let path = Path::new("/doc.md");
    let inner = Mem::new();
    publish(&inner, path, b"original");
    let vfs = SwappingReadVfs {
        inner,
        path: path.to_path_buf(),
        swap_at: 1,
        swap_to: b"a transient external write".to_vec(),
        read_calls: AtomicUsize::new(0),
    };
    let expect_hash = rune_db::hash_bytes(b"original");

    let outcome = run_materialize_vfs(
        &vfs,
        path,
        false,
        "new content",
        &expect_hash,
        None,
        SaveMode::Normal,
    );

    match outcome {
        MaterializeVfsOutcome::Committed { .. } => {}
        other => panic!("expected a transient mismatch to still commit, got {other:?}"),
    }
}

/// The mirror image: a mismatch that is STILL there on the re-verify read is
/// a real, stable conflict — reported as `Conflict`, never silently retried
/// forever.
#[test]
fn a_stable_cas_mismatch_reports_a_confirmed_conflict() {
    let path = Path::new("/doc.md");
    let vfs = Mem::new();
    publish(&vfs, path, b"external content, never ours");
    let expect_hash = rune_db::hash_bytes(b"original");

    let outcome = run_materialize_vfs(
        &vfs,
        path,
        false,
        "new content",
        &expect_hash,
        None,
        SaveMode::Normal,
    );

    match outcome {
        MaterializeVfsOutcome::Conflict {
            data, confirmed, ..
        } => {
            assert_eq!(data, b"external content, never ours");
            assert!(
                confirmed,
                "a quiescent Mem read must bracket-confirm even though it conflicts"
            );
        }
        other => panic!("expected a stable conflict, got {other:?}"),
    }
}

/// An ordinary committed save (no race at all) reports `confirmed: true` —
/// the post-publish bracketed stat holds stable on an otherwise-quiescent
/// `Mem`.
#[test]
fn an_ordinary_commit_reports_confirmed() {
    let path = Path::new("/doc.md");
    let vfs = Mem::new();
    publish(&vfs, path, b"original");
    let expect_hash = rune_db::hash_bytes(b"original");

    let outcome = run_materialize_vfs(
        &vfs,
        path,
        false,
        "new content",
        &expect_hash,
        None,
        SaveMode::Normal,
    );

    match outcome {
        MaterializeVfsOutcome::Committed { confirmed, .. } => assert!(confirmed),
        other => panic!("expected a plain commit, got {other:?}"),
    }
}
