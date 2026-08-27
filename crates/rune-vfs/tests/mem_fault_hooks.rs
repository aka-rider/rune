//! `Mem`'s fault-injection hooks (`fail_after`, `set_content_keep_identity`,
//! `mutate_after_next_stat`/`set_churning`, `fail_resolve`) are test-support
//! surface, but every hook's own plumbing must behave exactly as documented
//! — these tests exercise the hooks themselves, not just code that consumes
//! them.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io;
use std::path::Path;

use rune_vfs::{Mem, OpKind, Vfs, VfsTestExt};

#[test]
fn fail_after_only_fires_for_its_own_armed_op_kind() {
    let vfs = Mem::new();
    vfs.fail_after(OpKind::Exchange, io::ErrorKind::Other);

    // `rename_excl` also reaches `take_after_failure`, but the armed
    // failure targets `Exchange`, not `RenameExcl` — it must not fire here.
    let temp = vfs
        .write_durable(Path::new("/other.md"), b"x")
        .expect("write_durable");
    vfs.rename_excl(&temp, Path::new("/other.md"))
        .expect("an Exchange-armed fail_after must not affect RenameExcl");
}

#[test]
fn set_content_keep_identity_changes_content_without_minting_a_new_identity() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    vfs.save_atomic(path, b"before").expect("seed");
    let before = vfs.stat(path).expect("stat before");

    vfs.set_content_keep_identity(path, b"after".to_vec())
        .expect("set_content_keep_identity");

    assert_eq!(vfs.read(path).expect("read"), b"after");
    let after = vfs.stat(path).expect("stat after");
    assert_eq!(
        after.identity, before.identity,
        "the identity must be untouched by this hook"
    );
}

#[test]
fn churn_mode_mints_a_fresh_identity_and_tick_on_every_successive_stat() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    vfs.save_atomic(path, b"seed").expect("seed");
    vfs.set_churning(path, true);

    vfs.stat(path).expect("stat #1 (arms mutation #1)");
    let after_first = vfs.stat(path).expect("stat #2 (reflects mutation #1)");
    let content_after_first = vfs.read(path).expect("read after mutation #1");
    let after_second = vfs.stat(path).expect("stat #3 (reflects mutation #2)");

    assert_ne!(
        after_first.identity.inode, after_second.identity.inode,
        "each churn tick must mint a fresh inode, not reuse the last one"
    );
    assert_ne!(
        after_first.mtime, after_second.mtime,
        "each churn tick must advance the mod tick, not stay put"
    );
    assert!(
        content_after_first.starts_with(b"churn "),
        "churn mode must overwrite the content on every stat"
    );
}

#[test]
fn mutate_after_next_stat_only_fires_for_its_own_armed_path() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/a.md"), b"a-content").unwrap();
    vfs.save_atomic(Path::new("/b.md"), b"b-content").unwrap();
    vfs.mutate_after_next_stat(Path::new("/a.md"), b"mutated-a".to_vec());

    vfs.stat(Path::new("/b.md"))
        .expect("stat an unrelated path");
    assert_eq!(
        vfs.read(Path::new("/b.md")).unwrap(),
        b"b-content",
        "a stat on a path other than the armed one must not consume the armed mutation"
    );

    vfs.stat(Path::new("/a.md")).expect("stat the armed path");
    assert_eq!(
        vfs.read(Path::new("/a.md")).unwrap(),
        b"mutated-a",
        "the armed mutation must fire once its own path is stat'd"
    );
}

#[test]
fn fail_resolve_makes_every_future_resolve_of_that_path_fail() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    vfs.save_atomic(path, b"content").unwrap();

    vfs.fail_resolve(path);

    let err = vfs
        .resolve(path)
        .expect_err("fail_resolve must make resolve fail for the armed path");
    assert_ne!(err.kind(), io::ErrorKind::NotFound);
}
