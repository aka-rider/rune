//! Proves that capturing displaced bytes as a durable blob before they're
//! ever discarded is expressible against the split primitive set — the
//! exact property `save_atomic` alone cannot provide (see its doc comment
//! in `rune_vfs::Vfs`), and the reason WP1 exists.
//!
//! Sequence: write A to `path` directly (establishing the "existing file"
//! SWAP case), `write_durable` B to a fresh temp, `exchange(temp, path)`,
//! then read the temp back — it must hold A, not B, and not be gone.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rune_vfs::{Disk, Mem, Vfs};
use std::fs;
use std::path::PathBuf;

const A: &[u8] = b"the original content the caller must be able to recover";
const B: &[u8] = b"the new content being published over it";

#[test]
fn mem_exchange_preserves_displaced_bytes_for_the_caller_to_read() {
    let vfs = Mem::new();
    let path = PathBuf::from("/doc.md");

    // Establish A at `path` via the two-primitive publish (RenameExcl, since
    // `path` doesn't exist yet) so the subsequent exchange takes the real
    // SWAP shape a save-over-an-existing-file uses.
    let first_temp = vfs.write_durable(&path, A).expect("write_durable A");
    vfs.rename_excl(&first_temp, &path)
        .expect("publish A onto path");
    assert_eq!(vfs.read(&path).expect("read A"), A);

    // Write B to a fresh temp — `path` is untouched so far.
    let temp = vfs.write_durable(&path, B).expect("write_durable B");
    assert_eq!(vfs.read(&path).expect("path still holds A"), A);

    // Publish B by swapping it with `path`. Unlike `save_atomic`, nothing
    // here unlinks the displaced content.
    vfs.exchange(&temp, &path).expect("exchange temp <-> path");

    // `path` now holds the new content...
    assert_eq!(vfs.read(&path).expect("read path after exchange"), B);
    // ...and the temp — not deleted, not touched by the swap's own logic —
    // still holds exactly what the swap displaced: A.
    assert_eq!(
        vfs.read(&temp).expect("read temp after exchange"),
        A,
        "the swap must not destroy the bytes it displaced"
    );
}

#[test]
fn disk_exchange_preserves_displaced_bytes_for_the_caller_to_read() {
    let tmp = std::env::temp_dir().join(format!(
        "rune-vfs-capture-before-discard-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");

    let vfs = Disk;
    let path = tmp.join("doc.md");

    let first_temp = vfs.write_durable(&path, A).expect("write_durable A");
    vfs.rename_excl(&first_temp, &path)
        .expect("publish A onto path");
    assert_eq!(vfs.read(&path).expect("read A"), A);

    let temp = vfs.write_durable(&path, B).expect("write_durable B");
    assert_eq!(vfs.read(&path).expect("path still holds A"), A);

    vfs.exchange(&temp, &path).expect("exchange temp <-> path");

    assert_eq!(vfs.read(&path).expect("read path after exchange"), B);
    assert_eq!(
        vfs.read(&temp).expect("read temp after exchange"),
        A,
        "the swap must not destroy the bytes it displaced"
    );

    let _ = fs::remove_dir_all(&tmp);
}
