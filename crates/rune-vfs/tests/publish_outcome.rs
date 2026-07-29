//! WP1: the publish outcome is truthful (`save_atomic` no longer destroys
//! displaced bytes when the swap itself succeeded but a later step failed)
//! and `Mem` can express that scenario instead of being structurally
//! incapable of it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rune_vfs::{Mem, OpKind, Vfs, published_not_durable};
use std::io;
use std::path::PathBuf;

/// WP1.S1/S2/S5: a post-swap failure (the exchange took effect, but the
/// operation still reports an error — reproduced against `Mem` via the new
/// `fail_after` injection) must NOT destroy the temp: `save_atomic`'s error
/// carries the `published_not_durable` marker, the destination already
/// holds the caller's new bytes, and the temp (holding whatever the swap
/// displaced) is preserved rather than removed.
#[test]
fn save_atomic_post_swap_failure_keeps_the_temp_and_publishes_the_new_bytes() {
    let vfs = Mem::new();
    let path = PathBuf::from("/doc.md");

    vfs.save_atomic(&path, b"original").expect("seed the file");

    vfs.fail_after(OpKind::Exchange, io::ErrorKind::Other);
    let err = vfs
        .save_atomic(&path, b"replacement")
        .expect_err("the armed fail_after must surface as an error");

    assert!(
        published_not_durable(&err),
        "a post-swap failure must be marked published_not_durable"
    );

    // The swap already took effect: the destination holds the new bytes...
    assert_eq!(
        vfs.read(&path).expect("destination readable"),
        b"replacement"
    );
    // ...and the temp (named in the error message) still holds the
    // displaced original — proof it was NOT removed.
    let temp = vfs
        .debug_paths()
        .into_iter()
        .find(|p| p != &path)
        .expect("the temp must still be present, not discarded");
    assert_eq!(
        vfs.read(&temp).expect("temp still readable"),
        b"original",
        "the temp must still hold the displaced bytes save_atomic must not destroy"
    );
}

/// WP1.S1/S2 counterpart: a failure that happens BEFORE the publish takes
/// effect (the ordinary `fail_next` injection on `write_durable`, i.e. the
/// publish never ran at all) is the "nothing changed" case — the temp
/// holds nothing that isn't also still safe on the untouched destination,
/// so it is removed exactly as before, and the error is NOT marked
/// `published_not_durable`.
#[test]
fn save_atomic_pre_publish_failure_removes_the_temp_and_is_not_marked_published() {
    let vfs = Mem::new();
    let path = PathBuf::from("/doc.md");
    vfs.save_atomic(&path, b"original").expect("seed the file");

    vfs.fail_next_save(io::ErrorKind::Other);
    let err = vfs
        .save_atomic(&path, b"replacement")
        .expect_err("write_durable failure must surface");

    assert!(
        !published_not_durable(&err),
        "a pre-publish failure must not carry the published_not_durable marker"
    );
    assert_eq!(
        vfs.read(&path).expect("destination untouched"),
        b"original",
        "the publish never ran: the destination must be untouched"
    );
    assert!(
        vfs.debug_paths().into_iter().all(|p| p == path),
        "no temp residue must remain after a pre-publish failure"
    );
}

/// WP1.S3 (finding 3): `Mem::exchange(p, p)` must be a no-op success — the
/// old eager-double-remove implementation dropped the file entirely,
/// because the second `remove` of the SAME key always missed.
#[test]
fn exchange_same_path_preserves_the_file() {
    let vfs = Mem::new();
    let path = PathBuf::from("/doc.md");
    vfs.save_atomic(&path, b"content").expect("seed the file");

    vfs.exchange(&path, &path)
        .expect("exchanging a path with itself must be a no-op success");

    assert_eq!(
        vfs.read(&path).expect("file must still be readable"),
        b"content",
        "exchange(p, p) must not delete the file"
    );
}

/// `exchange(missing, missing)` must still error `NotFound` — the no-op
/// short-circuit only applies when the path actually exists.
#[test]
fn exchange_same_path_on_a_missing_file_errors_not_found() {
    let vfs = Mem::new();
    let path = PathBuf::from("/missing.md");
    let err = vfs
        .exchange(&path, &path)
        .expect_err("exchanging a missing path with itself must still error");
    assert_eq!(err.kind(), io::ErrorKind::NotFound);
}

/// WP1.S6 (finding 10): `Mem::resolve` normalizes lexically, so two
/// spellings of the same path collapse to the same key.
#[test]
fn mem_resolve_collapses_dot_and_dotdot_components() {
    let vfs = Mem::new();
    let direct = vfs
        .resolve(&PathBuf::from("/a/b.md"))
        .expect("resolve direct");
    let dotted = vfs
        .resolve(&PathBuf::from("/a/./x/../b.md"))
        .expect("resolve dotted");
    assert_eq!(direct, dotted);

    let relative = vfs
        .resolve(&PathBuf::from("b.md"))
        .expect("resolve relative");
    assert_eq!(relative, PathBuf::from("/b.md"));
}

/// WP1.S6 (finding 9): `Mem::stat`'s `nlink` is settable, so the
/// hardlink-fork warning path has a test double capable of exercising
/// `nlink > 1` (previously hardcoded to `Some(1)`, untestable).
#[test]
fn mem_set_nlink_is_reflected_in_stat() {
    let vfs = Mem::new();
    let path = PathBuf::from("/doc.md");
    vfs.save_atomic(&path, b"content").expect("seed the file");

    let before = vfs.stat(&path).expect("stat before");
    assert_eq!(before.nlink, Some(1));

    vfs.set_nlink(&path, 2).expect("set_nlink");
    let after = vfs.stat(&path).expect("stat after");
    assert_eq!(after.nlink, Some(2));
}
