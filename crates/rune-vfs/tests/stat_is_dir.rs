//! `Vfs::stat`'s `is_dir` field — a regular file must report `false`, a
//! directory must report `true`, for both `Disk` and `Mem`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rune_vfs::{Disk, Mem, Vfs};
use std::fs;
use std::path::PathBuf;

// ============================================================================
// Disk
// ============================================================================

#[test]
fn disk_stat_distinguishes_file_from_dir() {
    let tmp = std::env::temp_dir().join(format!("rune-vfs-stat-isdir-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");

    let file_path = tmp.join("leaf.md");
    fs::write(&file_path, b"content").expect("write leaf.md");
    let dir_path = tmp.join("subdir");
    fs::create_dir(&dir_path).expect("mkdir subdir");

    let vfs = Disk;
    let file_stat = vfs.stat(&file_path).expect("stat file should succeed");
    assert!(
        !file_stat.is_dir,
        "a regular file must report is_dir == false"
    );

    let dir_stat = vfs.stat(&dir_path).expect("stat dir should succeed");
    assert!(dir_stat.is_dir, "a directory must report is_dir == true");

    let _ = fs::remove_dir_all(&tmp);
}

// ============================================================================
// Mem
// ============================================================================

#[test]
fn mem_stat_distinguishes_file_from_dir() {
    let vfs = Mem::new();
    vfs.save_atomic(&PathBuf::from("/a/leaf.md"), b"content")
        .expect("save leaf.md");

    let file_stat = vfs
        .stat(&PathBuf::from("/a/leaf.md"))
        .expect("stat file should succeed");
    assert!(
        !file_stat.is_dir,
        "a regular file must report is_dir == false"
    );

    // `/a` has no direct key of its own, only the descendant key
    // `/a/leaf.md` — so it's a synthetic directory the same way `read_dir`
    // derives one.
    let dir_stat = vfs
        .stat(&PathBuf::from("/a"))
        .expect("stat synthetic dir should succeed");
    assert!(
        dir_stat.is_dir,
        "a synthetic directory must report is_dir == true"
    );
}
