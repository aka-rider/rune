//! VFS round-trip tests — byte-identical save and read through both
//! `Disk` and `Mem` backends, plus error-path and residue checks.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rune_vfs::{Disk, Mem, Vfs};
use std::fs;
use std::io;
use std::path::PathBuf;

/// The three byte fixtures required by the spec.
fn fixtures() -> Vec<(String, Vec<u8>)> {
    vec![
        // (a) UTF-8 BOM prefix + text
        (
            "bom".to_string(),
            [0xEF, 0xBB, 0xBF, b'h', b'e', b'l', b'l', b'o'].to_vec(),
        ),
        // (b) CRLF line endings
        ("crlf".to_string(), b"line1\r\nline2\r\nline3\r\n".to_vec()),
        // (c) No trailing newline
        ("no_nl".to_string(), b"no trailing newline".to_vec()),
    ]
}

/// Run a round-trip test on both backends.
fn roundtrip(vfs: &impl Vfs, name: &str, bytes: &[u8]) {
    let path = PathBuf::from(name);
    vfs.save_atomic(&path, bytes).expect("save should succeed");
    let read_back = vfs.read(&path).expect("read should succeed");
    assert_eq!(
        &read_back, bytes,
        "round-trip {name}: saved bytes differ from input"
    );
}

#[test]
fn mem_roundtrip_all_fixtures() {
    let vfs = Mem::new();
    for (name, bytes) in fixtures() {
        roundtrip(&vfs, &name, &bytes);
    }
}

#[test]
fn disk_roundtrip_all_fixtures() {
    let tmp = std::env::temp_dir().join(format!("rune-vfs-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");

    let vfs = Disk;
    for (name, bytes) in fixtures() {
        let path = tmp.join(&name);
        vfs.save_atomic(&path, &bytes).expect("save should succeed");
        let read_back = vfs.read(&path).expect("read should succeed");
        assert_eq!(
            &read_back, &bytes,
            "disk round-trip {name}: saved bytes differ from input"
        );
    }

    // Cleanup
    let _ = fs::remove_dir_all(&tmp);
}

/// Save over an existing file should swap content (not append).
#[test]
fn mem_save_overwrites_existing() {
    let vfs = Mem::new();
    let path = PathBuf::from("overwrite_test");
    let original = b"first content".to_vec();
    let replacement = b"second content".to_vec();

    vfs.save_atomic(&path, &original).expect("first save ok");
    vfs.save_atomic(&path, &replacement)
        .expect("second save ok");

    let result = vfs.read(&path).expect("read ok");
    assert_eq!(
        &result, &replacement,
        "overwrite should replace, not append"
    );
    assert_eq!(result.len(), replacement.len());
}

#[test]
fn disk_save_overwrites_existing() {
    let tmp = std::env::temp_dir().join(format!("rune-vfs-overwrite-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");

    let vfs = Disk;
    let path = tmp.join("overwrite_test");
    let original = b"first content".to_vec();
    let replacement = b"second content".to_vec();

    vfs.save_atomic(&path, &original).expect("first save ok");
    vfs.save_atomic(&path, &replacement)
        .expect("second save ok");

    let result = vfs.read(&path).expect("read ok");
    assert_eq!(
        &result, &replacement,
        "overwrite should replace, not append"
    );
    assert_eq!(result.len(), replacement.len());

    let _ = fs::remove_dir_all(&tmp);
}

/// After a successful save, no `.rune-tmp*` residue should remain (EXCL path).
#[test]
fn disk_no_temp_residue_excl_path() {
    let tmp = std::env::temp_dir().join(format!("rune-vfs-residue-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");

    let vfs = Disk;
    let path = tmp.join("residue_test");
    vfs.save_atomic(&path, b"hello").expect("save ok");

    let entries: Vec<_> = fs::read_dir(&tmp)
        .expect("read dir ok")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    let tmp_files: Vec<_> = entries
        .iter()
        .filter(|n| n.contains(".rune-tmp-"))
        .collect();

    assert!(
        tmp_files.is_empty(),
        "found temp residue after successful save (EXCL): {tmp_files:?}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// After a successful save over an existing file, no `.rune-tmp*` residue (SWAP path).
#[test]
fn disk_no_temp_residue_swap_path() {
    let tmp = std::env::temp_dir().join(format!("rune-vfs-residue-swap-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");

    let vfs = Disk;
    let path = tmp.join("swap_test");

    // Create the destination first.
    vfs.save_atomic(&path, b"original")
        .expect("initial save ok");

    // Overwrite it (triggers SWAP path).
    vfs.save_atomic(&path, b"replacement")
        .expect("swap save ok");

    let entries: Vec<_> = fs::read_dir(&tmp)
        .expect("read dir ok")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();

    let tmp_files: Vec<_> = entries
        .iter()
        .filter(|n| n.contains(".rune-tmp-"))
        .collect();

    assert!(
        tmp_files.is_empty(),
        "found temp residue after successful save (SWAP): {tmp_files:?}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// `fail_next_save` fires exactly once, then clears.
#[test]
fn mem_fail_next_save_once() {
    let vfs = Mem::new();
    let path = PathBuf::from("fail_test");

    // First save should fail.
    vfs.fail_next_save(io::ErrorKind::Other);
    let err = vfs.save_atomic(&path, b"data");
    assert!(
        err.is_err(),
        "first save should have failed (fail_next_save)"
    );

    // Second save should succeed.
    let result = vfs.save_atomic(&path, b"data");
    assert!(
        result.is_ok(),
        "second save should succeed after fail_next cleared"
    );

    let content = vfs.read(&path).expect("read after fail_next should work");
    assert_eq!(&content, b"data");
}

/// `fail_next_save` with different error kinds.
#[test]
fn mem_fail_next_save_error_kind() {
    let vfs = Mem::new();
    let path = PathBuf::from("fail_kind_test");

    vfs.fail_next_save(io::ErrorKind::PermissionDenied);
    let err = vfs.save_atomic(&path, b"data");
    assert!(err.is_err());
    assert_eq!(err.unwrap_err().kind(), io::ErrorKind::PermissionDenied);

    // Should succeed after the one-shot failure.
    vfs.save_atomic(&path, b"data").expect("should succeed");
}

// ============================================================================
// REGRESSION TESTS for review fixes
// ============================================================================

/// Fix 1: Pre-existing temp file from crashed save must not be deleted on failure.
/// Create a temp file manually, then attempt save to a path that would use that temp.
/// The temp should still exist after the failed save.
#[test]
fn disk_regression_preexisting_temp_not_deleted_on_conflict() {
    let tmp = std::env::temp_dir().join(format!("rune-vfs-preexist-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");

    let vfs = Disk;
    let path = tmp.join("doc.md");
    let pid = std::process::id();
    let temp_path = tmp.join(format!(".doc.md.rune-tmp-{}", pid));

    // Pre-create a temp file (simulating crash residue).
    fs::write(&temp_path, b"old displaced bytes").expect("write pre-existing temp");

    // Attempt to save: create_new should fail with EEXIST.
    let result = vfs.save_atomic(&path, b"new content");
    assert!(
        result.is_err(),
        "save should fail when temp already exists (EEXIST)"
    );

    // The pre-existing temp file must still exist (not deleted).
    assert!(
        temp_path.exists(),
        "pre-existing temp file should NOT be deleted on conflict"
    );

    // Verify its content is unchanged.
    let content = fs::read(&temp_path).expect("read pre-existing temp");
    assert_eq!(
        &content, b"old displaced bytes",
        "pre-existing temp content must be preserved"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// Fix 3: Symlink targets must be written to, not the symlink replaced.
/// Create a symlink pointing to a real file, save through the symlink,
/// verify the real file got the bytes and the symlink is unchanged.
#[test]
fn disk_regression_symlink_resolves_to_target() {
    let tmp = std::env::temp_dir().join(format!("rune-vfs-symlink-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");

    let vfs = Disk;
    let real_file = tmp.join("real.md");
    let symlink = tmp.join("link.md");

    // Create a real file and a symlink to it.
    fs::write(&real_file, b"original").expect("write real file");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real_file, &symlink).expect("create symlink");

    // Save through the symlink.
    vfs.save_atomic(&symlink, b"new content")
        .expect("save through symlink");

    // The symlink must still be a symlink.
    let symlink_metadata = fs::symlink_metadata(&symlink).expect("read symlink metadata");
    assert!(
        symlink_metadata.file_type().is_symlink(),
        "symlink should still be a symlink after save"
    );

    // The real file must have the new content.
    let real_content = fs::read(&real_file).expect("read real file");
    assert_eq!(
        &real_content, b"new content",
        "real file must have the new content written through symlink"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// Fix 6: Bare relative filenames must not fail on parent fsync.
/// Save to a relative path (not absolute), verify it succeeds and parent fsync doesn't panic.
#[test]
fn disk_regression_bare_relative_filename() {
    let tmp = std::env::temp_dir().join(format!("rune-vfs-relative-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");

    // Change to the temp directory to test a truly relative path.
    let old_cwd = std::env::current_dir().expect("get current dir");
    std::env::set_current_dir(&tmp).expect("change to temp dir");

    let vfs = Disk;
    let relative_path = PathBuf::from("relative_file.md");

    // This should succeed; parent() -> Some("") for a bare filename,
    // and the code should fsync "." instead of skipping.
    vfs.save_atomic(&relative_path, b"content from relative path")
        .expect("save to relative path should succeed");

    // Verify the file was created in the temp directory.
    let file_path = tmp.join("relative_file.md");
    assert!(
        file_path.exists(),
        "file should exist in the temp directory"
    );

    let content = fs::read(&file_path).expect("read relative file");
    assert_eq!(
        &content, b"content from relative path",
        "content must be correct"
    );

    // Restore the original working directory.
    std::env::set_current_dir(&old_cwd).expect("restore current dir");

    let _ = fs::remove_dir_all(&tmp);
}
