//! `resolve_or_missing`'s NotFound-shaped-error guard, and `put_if_match`'s
//! confirmed/matching retry decision — split out of
//! `publish_conditions_tests.rs` to keep that file under the file-size
//! ceiling.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::sighting::BRACKET_MAX_ATTEMPTS;
use crate::{Disk, Identity, Mem};
use std::sync::atomic::{AtomicU64, Ordering};

fn publish_direct(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

#[test]
fn if_match_over_a_missing_ancestor_directory_reports_missing_not_an_error() {
    let tmp = std::env::temp_dir().join(format!(
        "rune-vfs-if-match-missing-ancestor-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).expect("create scratch dir");
    let path = tmp.join("no-such-subdir").join("doc.md");

    let outcome = put(
        &Disk,
        &path,
        b"new",
        PutCondition::IfMatch(etag_of(b"whatever")),
    )
    .expect("a missing ancestor directory must resolve to Missing, not an error");

    assert!(matches!(outcome, PutOutcome::Missing));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn if_match_propagates_a_resolve_failure_that_is_not_shaped_like_not_found() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    vfs.fail_resolve(path);

    let err = put(
        &vfs,
        path,
        b"new",
        PutCondition::IfMatch(etag_of(b"whatever")),
    )
    .unwrap_err();

    assert_ne!(
        err.kind(),
        io::ErrorKind::NotFound,
        "fail_resolve's error is not NotFound-shaped, so it must propagate as an error \
         rather than being folded into Missing"
    );
}

struct ReadCountingVfs {
    inner: Mem,
    read_calls: AtomicU64,
}

impl Vfs for ReadCountingVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.read_calls.fetch_add(1, Ordering::SeqCst);
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
    fn stat(&self, path: &Path) -> io::Result<crate::Stat> {
        self.inner.stat(path)
    }
    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.resolve(path)
    }
    fn mkdir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.mkdir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<crate::DirEntry>> {
        self.inner.read_dir(path)
    }
    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.read_link(path)
    }
}

#[test]
fn if_match_on_a_quiescent_confirmed_match_never_re_fetches() {
    let inner = Mem::new();
    publish_direct(&inner, Path::new("/doc.md"), b"original");
    let vfs = ReadCountingVfs {
        inner,
        read_calls: AtomicU64::new(0),
    };

    let outcome = put(
        &vfs,
        Path::new("/doc.md"),
        b"updated",
        PutCondition::IfMatch(etag_of(b"original")),
    )
    .unwrap();

    assert!(matches!(
        outcome,
        PutOutcome::Committed { durable: true, .. }
    ));
    assert_eq!(
        vfs.read_calls.load(Ordering::SeqCst),
        2,
        "a confirmed, matching sighting must be fetched exactly once (plus the one \
         displaced-bytes read every commit does) — never re-fetched"
    );
}

struct FlappingIdentityReadCountingVfs {
    inner: Mem,
    stat_calls: AtomicU64,
    read_calls: AtomicU64,
}

impl Vfs for FlappingIdentityReadCountingVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.read_calls.fetch_add(1, Ordering::SeqCst);
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
    fn stat(&self, path: &Path) -> io::Result<crate::Stat> {
        let mut stat = self.inner.stat(path)?;
        let n = self.stat_calls.fetch_add(1, Ordering::SeqCst);
        stat.identity = Identity {
            inode: Some(n),
            device: Some(1),
        };
        Ok(stat)
    }
    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.resolve(path)
    }
    fn mkdir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.mkdir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<crate::DirEntry>> {
        self.inner.read_dir(path)
    }
    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.read_link(path)
    }
}

#[test]
fn if_match_on_an_unconfirmed_but_matching_sighting_retries_exactly_once() {
    let inner = Mem::new();
    publish_direct(&inner, Path::new("/doc.md"), b"stable content");
    let vfs = FlappingIdentityReadCountingVfs {
        inner,
        stat_calls: AtomicU64::new(0),
        read_calls: AtomicU64::new(0),
    };

    let outcome = put(
        &vfs,
        Path::new("/doc.md"),
        b"new",
        PutCondition::IfMatch(etag_of(b"stable content")),
    )
    .unwrap();

    assert!(matches!(outcome, PutOutcome::Conflict { .. }));
    assert_eq!(
        vfs.read_calls.load(Ordering::SeqCst),
        u64::from(2 * BRACKET_MAX_ATTEMPTS),
        "an unconfirmed sighting, even with a matching hash, must trigger exactly one \
         retry re-fetch — each of the two fetches exhausts its own bracket's retry ceiling \
         since identity never stops flapping"
    );
}
