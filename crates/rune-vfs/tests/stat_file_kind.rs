//! `Vfs::stat`'s `kind` field — a regular file must report `FileKind::File`,
//! a directory `FileKind::Dir`, for both `Disk` and `Mem`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rune_vfs::{Disk, FileKind, Mem, Vfs};
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
        file_stat.kind == FileKind::File,
        "a regular file must report FileKind::File"
    );

    let dir_stat = vfs.stat(&dir_path).expect("stat dir should succeed");
    assert!(
        dir_stat.kind == FileKind::Dir,
        "a directory must report FileKind::Dir"
    );

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
        file_stat.kind == FileKind::File,
        "a regular file must report FileKind::File"
    );

    // `/a` has no direct key of its own, only the descendant key
    // `/a/leaf.md` — so it's a synthetic directory the same way `read_dir`
    // derives one.
    let dir_stat = vfs
        .stat(&PathBuf::from("/a"))
        .expect("stat synthetic dir should succeed");
    assert!(
        dir_stat.kind == FileKind::Dir,
        "a synthetic directory must report is_dir == true"
    );
}

/// A FIFO is neither a file nor a directory. This matters beyond taxonomy:
/// the editor's open path reads synchronously, so offering a FIFO as a link
/// target would block it forever with the buffer unsaved.
#[test]
fn disk_stat_reports_a_fifo_as_other() {
    let tmp = std::env::temp_dir().join(format!("rune-vfs-stat-fifo-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");
    let fifo = tmp.join("pipe");

    let made = std::process::Command::new("/usr/bin/mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(made.success(), "mkfifo should succeed");

    let stat = Disk.stat(&fifo).expect("stat fifo should succeed");
    assert_eq!(
        stat.kind,
        FileKind::Other,
        "a FIFO must report FileKind::Other, never File"
    );

    let _ = fs::remove_dir_all(&tmp);
}
