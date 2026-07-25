//! VFS round-trip tests — byte-identical save and read through both
//! `Disk` and `Mem` backends, plus error-path and residue checks.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rune_core::buffer::Buffer;
use rune_core::vfs::{Disk, Mem, Vfs};
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

/// After a successful save, no `.rune-tmp*` residue should remain.
#[test]
fn disk_no_temp_residue() {
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
        "found temp residue after successful save: {tmp_files:?}"
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

/// `Buffer::from_bytes` refuses invalid UTF-8.
#[test]
fn buffer_refuses_invalid_utf8() {
    let result = Buffer::from_bytes(vec![0xff, 0xfe]);
    assert!(result.is_err(), "invalid UTF-8 should be refused");
}
