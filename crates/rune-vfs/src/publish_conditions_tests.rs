//! `if_match`/`if_absent`/TOCTOU publish tests — split out of
//! `publish_tests.rs` to keep that file under the file-size ceiling.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::{Identity, Mem, OpKind};
use std::sync::atomic::{AtomicU64, Ordering};

fn publish_direct(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

#[test]
fn if_match_conflict_on_hash_mismatch() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish_direct(&vfs, path, b"original");

    let outcome = put(&vfs, path, b"new", PutCondition::IfMatch(etag_of(b"wrong"))).unwrap();
    assert!(matches!(outcome, PutOutcome::Conflict { .. }));
}

#[test]
fn if_match_missing_destination_returns_missing() {
    let vfs = Mem::new();
    let outcome = put(
        &vfs,
        Path::new("/gone.md"),
        b"new",
        PutCondition::IfMatch(etag_of(b"whatever")),
    )
    .unwrap();
    assert!(matches!(outcome, PutOutcome::Missing));
}

#[test]
fn if_match_committed_replaces_matching_content() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish_direct(&vfs, path, b"original");

    let outcome = put(
        &vfs,
        path,
        b"updated",
        PutCondition::IfMatch(etag_of(b"original")),
    )
    .unwrap();
    assert!(matches!(
        outcome,
        PutOutcome::Committed { durable: true, .. }
    ));
    assert_eq!(vfs.read(path).unwrap(), b"updated");
}

struct FlappingIdentityVfs {
    inner: Mem,
    calls: AtomicU64,
}

impl Vfs for FlappingIdentityVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<std::path::PathBuf> {
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
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        stat.identity = Identity {
            inode: Some(n),
            device: Some(1),
        };
        Ok(stat)
    }
    fn resolve(&self, path: &Path) -> io::Result<std::path::PathBuf> {
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
fn if_match_refuses_an_unconfirmed_read_even_when_the_hash_matches() {
    let inner = Mem::new();
    publish_direct(&inner, Path::new("/doc.md"), b"stable content");
    let vfs = FlappingIdentityVfs {
        inner,
        calls: AtomicU64::new(0),
    };

    let outcome = put(
        &vfs,
        Path::new("/doc.md"),
        b"new",
        PutCondition::IfMatch(etag_of(b"stable content")),
    )
    .unwrap();
    assert!(matches!(outcome, PutOutcome::Conflict { .. }));
}

#[test]
fn a_displaced_read_failure_after_the_publish_is_still_a_commit_and_keeps_the_temp() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish_direct(&vfs, path, b"original");
    vfs.fail_next(OpKind::Read, io::ErrorKind::PermissionDenied);

    let outcome = put(&vfs, path, b"updated", PutCondition::Force { expect: None }).unwrap();
    let PutOutcome::Committed {
        durable: true,
        stray_temp,
        ..
    } = &outcome
    else {
        unreachable!("expected a durable Committed, got {outcome:?}");
    };
    assert_eq!(vfs.read(path).unwrap(), b"updated");
    let paths = vfs.debug_paths();
    assert_eq!(
        paths.len(),
        2,
        "the temp may hold the sole displaced copy and must survive"
    );
    let stray_temp = stray_temp
        .as_deref()
        .expect("a kept temp must be named on the outcome");
    assert!(
        paths.iter().any(|p| p == stray_temp),
        "the named stray_temp must be the surviving temp file"
    );
    assert_ne!(stray_temp, path);
}

#[test]
fn a_flapping_post_publish_stat_reports_stat_unconfirmed() {
    let inner = Mem::new();
    let path = Path::new("/doc.md");
    publish_direct(&inner, path, b"original");
    let vfs = FlappingIdentityVfs {
        inner,
        calls: AtomicU64::new(0),
    };

    let outcome = put(
        &vfs,
        path,
        b"updated",
        PutCondition::Force {
            expect: Some(etag_of(b"original")),
        },
    )
    .unwrap();
    let PutOutcome::Committed { sighted, .. } = outcome else {
        unreachable!("expected Committed, got {outcome:?}");
    };
    assert!(!sighted.is_confirmed());
}

#[test]
fn a_quiescent_commit_reports_a_confirmed_post_publish_stat() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish_direct(&vfs, path, b"original");

    let outcome = put(
        &vfs,
        path,
        b"updated",
        PutCondition::IfMatch(etag_of(b"original")),
    )
    .unwrap();
    let PutOutcome::Committed { sighted, .. } = outcome else {
        unreachable!("expected Committed, got {outcome:?}");
    };
    assert!(sighted.is_confirmed());
}

#[test]
fn if_absent_loser_gets_conflict_and_the_temp_is_removed() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish_direct(&vfs, path, b"winner");

    let before = vfs.debug_paths().len();
    let outcome = put(&vfs, path, b"loser", PutCondition::IfAbsent).unwrap();
    let PutOutcome::Conflict { current, .. } = outcome else {
        unreachable!("expected Conflict, got {outcome:?}");
    };
    assert_eq!(current.bytes, b"winner");
    assert_eq!(vfs.debug_paths().len(), before);
}

#[test]
fn if_absent_non_collision_failure_cleans_up_the_temp() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    vfs.fail_next(OpKind::RenameExcl, io::ErrorKind::PermissionDenied);

    let before = vfs.debug_paths().len();
    let result = put(&vfs, path, b"bytes", PutCondition::IfAbsent);
    assert!(result.is_err());
    assert_eq!(
        vfs.debug_paths().len(),
        before,
        "a non-collision publish failure must not leak the temp, matching put_force's policy"
    );
}

struct FailRenameExclAndRemoveVfs {
    inner: Mem,
}

impl Vfs for FailRenameExclAndRemoveVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        self.inner.write_durable(path, bytes)
    }
    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        self.inner.exchange(a, b)
    }
    fn rename_excl(&self, _old: &Path, _new: &Path) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "rename_excl always fails",
        ))
    }
    fn remove(&self, _path: &Path) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "remove always fails",
        ))
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
fn if_absent_non_collision_failure_notes_a_cleanup_failure_too() {
    let vfs = FailRenameExclAndRemoveVfs { inner: Mem::new() };
    let path = Path::new("/doc.md");

    let before = vfs.inner.debug_paths().len();
    let err = put(&vfs, path, b"bytes", PutCondition::IfAbsent).unwrap_err();
    assert_eq!(
        vfs.inner.debug_paths().len(),
        before + 1,
        "the temp survives when its own cleanup also fails"
    );
    assert!(err.to_string().contains("could not be cleaned up"));
}

#[test]
fn if_match_over_a_directory_refuses_with_is_a_directory() {
    let vfs = Mem::new();
    publish_direct(&vfs, Path::new("/a/b.md"), b"content");

    let result = put(
        &vfs,
        Path::new("/a"),
        b"anything",
        PutCondition::IfMatch(etag_of(b"anything")),
    );
    let err = result.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::IsADirectory);
}

struct ResolveCountingVfs {
    inner: Mem,
    resolve_calls: AtomicU64,
}

impl Vfs for ResolveCountingVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<std::path::PathBuf> {
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
    fn resolve(&self, path: &Path) -> io::Result<std::path::PathBuf> {
        self.resolve_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.resolve(path)
    }
    fn mkdir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.mkdir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<crate::DirEntry>> {
        self.inner.read_dir(path)
    }
    fn read_link(&self, path: &Path) -> io::Result<std::path::PathBuf> {
        self.inner.read_link(path)
    }
}

#[test]
fn if_match_resolves_the_path_exactly_once() {
    let inner = Mem::new();
    publish_direct(&inner, Path::new("/doc.md"), b"original");
    let vfs = ResolveCountingVfs {
        inner,
        resolve_calls: AtomicU64::new(0),
    };

    let outcome = put(
        &vfs,
        Path::new("/doc.md"),
        b"updated",
        PutCondition::IfMatch(etag_of(b"original")),
    )
    .unwrap();

    assert!(matches!(outcome, PutOutcome::Committed { .. }));
    assert_eq!(
        vfs.resolve_calls.load(Ordering::SeqCst),
        1,
        "the etag check and the publish must share one resolution of the path"
    );
}

#[test]
fn if_absent_conflict_resolves_the_path_exactly_once() {
    let inner = Mem::new();
    publish_direct(&inner, Path::new("/doc.md"), b"winner");
    let vfs = ResolveCountingVfs {
        inner,
        resolve_calls: AtomicU64::new(0),
    };

    let outcome = put(&vfs, Path::new("/doc.md"), b"loser", PutCondition::IfAbsent).unwrap();

    assert!(matches!(outcome, PutOutcome::Conflict { .. }));
    assert_eq!(
        vfs.resolve_calls.load(Ordering::SeqCst),
        1,
        "reporting the conflict winner must reuse the same resolution, not re-resolve"
    );
}
