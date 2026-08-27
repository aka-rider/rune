#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::{Mem, OpKind};

fn publish_direct(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

#[test]
fn force_committed_reports_durable_false_on_a_post_publish_durability_failure() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish_direct(&vfs, path, b"original");
    vfs.fail_after(OpKind::Exchange, io::ErrorKind::Other);

    let outcome = put(
        &vfs,
        path,
        b"original",
        PutCondition::Force { expect: None },
    )
    .unwrap();
    assert!(matches!(
        outcome,
        PutOutcome::Committed { durable: false, .. }
    ));
    assert_eq!(vfs.read(path).unwrap(), b"original");
    assert_eq!(
        vfs.debug_paths().len(),
        2,
        "an unconfirmed-durability publish must keep the sibling temp"
    );
}

#[test]
fn a_durable_commit_leaves_no_temp_residue() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish_direct(&vfs, path, b"original");

    let outcome = put(
        &vfs,
        path,
        b"updated",
        PutCondition::Force {
            expect: Some(etag_of(b"original")),
        },
    )
    .unwrap();
    assert!(matches!(
        outcome,
        PutOutcome::Committed { durable: true, .. }
    ));
    assert_eq!(vfs.debug_paths().len(), 1);
}

#[test]
fn force_with_matching_expect_commits() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish_direct(&vfs, path, b"original");

    let outcome = put(
        &vfs,
        path,
        b"updated",
        PutCondition::Force {
            expect: Some(etag_of(b"original")),
        },
    )
    .unwrap();
    assert!(matches!(
        outcome,
        PutOutcome::Committed { durable: true, .. }
    ));
    assert_eq!(vfs.read(path).unwrap(), b"updated");
}

#[test]
fn force_over_foreign_bytes_races_with_displaced_captured() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish_direct(&vfs, path, b"original");
    // A foreign writer replaces the content out from under the caller's
    // recorded baseline before the Force publish runs.
    let foreign_temp = vfs.write_durable(path, b"foreign").unwrap();
    vfs.exchange(&foreign_temp, path).unwrap();
    let _ = vfs.remove(&foreign_temp);

    let outcome = put(
        &vfs,
        path,
        b"mine",
        PutCondition::Force {
            expect: Some(etag_of(b"original")),
        },
    )
    .unwrap();
    let PutOutcome::Raced { displaced, .. } = outcome else {
        unreachable!("expected Raced, got {outcome:?}");
    };
    assert_eq!(displaced.bytes, b"foreign");
    assert_eq!(vfs.read(path).unwrap(), b"mine");
}

#[test]
fn force_fresh_create_commits() {
    let vfs = Mem::new();
    let outcome = put(
        &vfs,
        Path::new("/new.md"),
        b"content",
        PutCondition::Force { expect: None },
    )
    .unwrap();
    assert!(matches!(
        outcome,
        PutOutcome::Committed { durable: true, .. }
    ));
}

#[test]
fn two_sequential_puts_to_one_path_never_collide_on_temp_names() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish_direct(&vfs, path, b"one");

    put(&vfs, path, b"two", PutCondition::Force { expect: None }).unwrap();
    put(&vfs, path, b"three", PutCondition::Force { expect: None }).unwrap();

    assert_eq!(vfs.read(path).unwrap(), b"three");
}

#[test]
fn force_over_existing_propagates_a_publish_failure_that_never_took_effect() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish_direct(&vfs, path, b"original");
    // A pre-publish failure (`fail_next`, not `fail_after`) fires before the
    // exchange takes effect, so it must NOT carry the `published_not_durable`
    // marker `finish_over_existing`'s guard checks for — it must propagate
    // as a plain `Err`, never be folded into a `durable: false` commit.
    vfs.fail_next(OpKind::Exchange, io::ErrorKind::PermissionDenied);

    let err = put(
        &vfs,
        path,
        b"updated",
        PutCondition::Force {
            expect: Some(etag_of(b"original")),
        },
    )
    .unwrap_err();

    assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(
        vfs.read(path).unwrap(),
        b"original",
        "the publish never took effect: the destination must be untouched"
    );
}

#[test]
fn force_over_a_directory_refuses_and_leaves_it_intact() {
    let vfs = Mem::new();
    publish_direct(&vfs, Path::new("/notes/a.md"), b"content");
    let before = vfs.debug_paths().len();

    let err = put(
        &vfs,
        Path::new("/notes"),
        b"anything",
        PutCondition::Force { expect: None },
    )
    .unwrap_err();

    assert_eq!(err.kind(), io::ErrorKind::IsADirectory);
    assert_eq!(vfs.stat(Path::new("/notes")).unwrap().kind, FileKind::Dir);
    assert_eq!(vfs.read(Path::new("/notes/a.md")).unwrap(), b"content");
    assert_eq!(
        vfs.debug_paths().len(),
        before,
        "no stray temp must remain after the refusal"
    );
}
