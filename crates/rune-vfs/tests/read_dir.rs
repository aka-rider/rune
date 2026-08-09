//! `Vfs::read_dir` tests — direct-children enumeration, sorted
//! directories-first then case-INsensitively by name (ties broken by exact,
//! original-case name), for both `Disk` and `Mem`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rune_vfs::{DirEntry, Disk, Mem, Vfs};
use std::fs;
use std::path::PathBuf;

// ============================================================================
// Disk
// ============================================================================

#[test]
fn disk_read_dir_lists_children_sorted_dirs_first() {
    let tmp = std::env::temp_dir().join(format!("rune-vfs-readdir-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");

    // Files and dirs, deliberately created out of sorted order.
    fs::write(tmp.join("zeta.md"), b"z").expect("write zeta");
    fs::create_dir(tmp.join("beta")).expect("mkdir beta");
    fs::write(tmp.join("alpha.md"), b"a").expect("write alpha");
    fs::create_dir(tmp.join("Aardvark")).expect("mkdir Aardvark");

    let vfs = Disk;
    let entries = vfs.read_dir(&tmp).expect("read_dir should succeed");

    // Dirs first (case-insensitive by name: "aardvark" < "beta"), then
    // files (case-insensitive by name: "alpha.md" < "zeta.md") — this
    // fixture's names happen to sort the same way case-sensitively too;
    // see `mem_read_dir_sort_order_is_case_insensitive_with_a_mixed_case_tiebreak`
    // for a fixture where that would NOT hold.
    assert_eq!(
        entries,
        vec![
            DirEntry {
                name: "Aardvark".to_string(),
                path: tmp.join("Aardvark"),
                is_dir: true
            },
            DirEntry {
                name: "beta".to_string(),
                path: tmp.join("beta"),
                is_dir: true
            },
            DirEntry {
                name: "alpha.md".to_string(),
                path: tmp.join("alpha.md"),
                is_dir: false
            },
            DirEntry {
                name: "zeta.md".to_string(),
                path: tmp.join("zeta.md"),
                is_dir: false
            },
        ]
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn disk_read_dir_empty_dir() {
    let tmp = std::env::temp_dir().join(format!("rune-vfs-readdir-empty-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");

    let vfs = Disk;
    let entries = vfs.read_dir(&tmp).expect("read_dir should succeed");
    assert!(entries.is_empty(), "empty dir should yield no entries");

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn disk_read_dir_not_recursive() {
    let tmp =
        std::env::temp_dir().join(format!("rune-vfs-readdir-recursive-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("create temp dir");

    fs::create_dir(tmp.join("sub")).expect("mkdir sub");
    fs::write(tmp.join("sub").join("nested.md"), b"n").expect("write nested");
    fs::write(tmp.join("top.md"), b"t").expect("write top");

    let vfs = Disk;
    let entries = vfs.read_dir(&tmp).expect("read_dir should succeed");

    assert_eq!(
        entries,
        vec![
            DirEntry {
                name: "sub".to_string(),
                path: tmp.join("sub"),
                is_dir: true
            },
            DirEntry {
                name: "top.md".to_string(),
                path: tmp.join("top.md"),
                is_dir: false
            },
        ],
        "read_dir must list only direct children, not descend into `sub`"
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn disk_read_dir_missing_path_errors() {
    let tmp = std::env::temp_dir().join(format!("rune-vfs-readdir-missing-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let vfs = Disk;
    let err = vfs
        .read_dir(&tmp)
        .expect_err("read_dir on a nonexistent path must propagate an error");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

// ============================================================================
// Mem
// ============================================================================

/// WP1.S6 (finding 7): `Mem::read_dir` on a path with no exact key and no
/// descendant key must error `NotFound`, matching `Disk`'s behavior for a
/// genuinely nonexistent directory — it must not report an empty listing,
/// which used to be indistinguishable from a real, existing, empty
/// directory.
#[test]
fn mem_read_dir_errors_not_found_on_untouched_vfs() {
    let vfs = Mem::new();
    let err = vfs
        .read_dir(&PathBuf::from("/a"))
        .expect_err("read_dir on a path with no keys under it must error");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

/// The synthetic root always exists, even on a completely untouched `Mem` —
/// unlike any other path, it is never reported `NotFound`.
#[test]
fn mem_read_dir_root_always_exists() {
    let vfs = Mem::new();
    let entries = vfs
        .read_dir(&PathBuf::from("/"))
        .expect("the synthetic root always exists");
    assert!(entries.is_empty());
}

#[test]
fn mem_read_dir_files_at_root() {
    let vfs = Mem::new();
    vfs.save_atomic(&PathBuf::from("/one.md"), b"1")
        .expect("save one");
    vfs.save_atomic(&PathBuf::from("/two.md"), b"2")
        .expect("save two");

    let entries = vfs
        .read_dir(&PathBuf::from("/"))
        .expect("read_dir should succeed");
    assert_eq!(
        entries,
        vec![
            DirEntry {
                name: "one.md".to_string(),
                path: PathBuf::from("/one.md"),
                is_dir: false
            },
            DirEntry {
                name: "two.md".to_string(),
                path: PathBuf::from("/two.md"),
                is_dir: false
            },
        ]
    );
}

/// Keys `/a/b/c.md` and `/a/d.md`: under `/a`, `b` is a synthetic directory
/// (it has a descendant, `c.md`, one level further down) and `d.md` is a
/// file (a key exactly one component below `/a`).
#[test]
fn mem_read_dir_synthetic_dir_from_nested_key() {
    let vfs = Mem::new();
    vfs.save_atomic(&PathBuf::from("/a/b/c.md"), b"c")
        .expect("save c.md");
    vfs.save_atomic(&PathBuf::from("/a/d.md"), b"d")
        .expect("save d.md");

    let entries = vfs
        .read_dir(&PathBuf::from("/a"))
        .expect("read_dir should succeed");
    assert_eq!(
        entries,
        vec![
            DirEntry {
                name: "b".to_string(),
                path: PathBuf::from("/a/b"),
                is_dir: true
            },
            DirEntry {
                name: "d.md".to_string(),
                path: PathBuf::from("/a/d.md"),
                is_dir: false
            },
        ]
    );
}

/// Two files sharing the same nested parent (`/a/b/c.md`, `/a/b/e.md`) must
/// contribute exactly one synthetic `b` entry, not two.
#[test]
fn mem_read_dir_dedups_synthetic_dir_from_multiple_keys() {
    let vfs = Mem::new();
    vfs.save_atomic(&PathBuf::from("/a/b/c.md"), b"c")
        .expect("save c.md");
    vfs.save_atomic(&PathBuf::from("/a/b/e.md"), b"e")
        .expect("save e.md");
    vfs.save_atomic(&PathBuf::from("/a/b/deeper/f.md"), b"f")
        .expect("save deeper f.md");

    let entries = vfs
        .read_dir(&PathBuf::from("/a"))
        .expect("read_dir should succeed");
    assert_eq!(
        entries,
        vec![DirEntry {
            name: "b".to_string(),
            path: PathBuf::from("/a/b"),
            is_dir: true
        }],
        "one synthetic `b` entry, deduplicated across all keys under it"
    );
}

/// Sort order: directories before files, then case-INsensitively by name
/// within each group, ties broken by exact, original-case name. This
/// fixture's names happen to sort the same way case-sensitively
/// too; `mem_read_dir_sort_order_is_case_insensitive_with_a_mixed_case_tiebreak`
/// below covers a fixture where that would NOT hold.
#[test]
fn mem_read_dir_sort_order_dirs_first_then_case_insensitive_name() {
    let vfs = Mem::new();
    vfs.save_atomic(&PathBuf::from("/r/zeta.md"), b"z")
        .expect("save zeta");
    vfs.save_atomic(&PathBuf::from("/r/alpha.md"), b"a")
        .expect("save alpha");
    vfs.save_atomic(&PathBuf::from("/r/beta/nested.md"), b"n")
        .expect("save nested under beta");
    vfs.save_atomic(&PathBuf::from("/r/Aardvark/nested.md"), b"n")
        .expect("save nested under Aardvark");

    let entries = vfs
        .read_dir(&PathBuf::from("/r"))
        .expect("read_dir should succeed");
    assert_eq!(
        entries,
        vec![
            DirEntry {
                name: "Aardvark".to_string(),
                path: PathBuf::from("/r/Aardvark"),
                is_dir: true
            },
            DirEntry {
                name: "beta".to_string(),
                path: PathBuf::from("/r/beta"),
                is_dir: true
            },
            DirEntry {
                name: "alpha.md".to_string(),
                path: PathBuf::from("/r/alpha.md"),
                is_dir: false
            },
            DirEntry {
                name: "zeta.md".to_string(),
                path: PathBuf::from("/r/zeta.md"),
                is_dir: false
            },
        ]
    );
}

/// A fixture where case-sensitive and case-insensitive ordering actually
/// DISAGREE: `"Banana"` sorts before `"apple"` case-sensitively (ASCII 'B'
/// = 66 < 'a' = 97) but after it case-insensitively ("apple" < "banana").
/// Also covers the tie-break: `"File.md"` and `"file.md"` collide once
/// lowercased, so the exact (original-case) name breaks the tie
/// deterministically ('F' = 70 < 'f' = 102).
#[test]
fn mem_read_dir_sort_order_is_case_insensitive_with_a_mixed_case_tiebreak() {
    let vfs = Mem::new();
    vfs.save_atomic(&PathBuf::from("/r/Banana"), b"b")
        .expect("save Banana");
    vfs.save_atomic(&PathBuf::from("/r/apple"), b"a")
        .expect("save apple");
    vfs.save_atomic(&PathBuf::from("/r/file.md"), b"f")
        .expect("save file.md");
    vfs.save_atomic(&PathBuf::from("/r/File.md"), b"F")
        .expect("save File.md");

    let entries = vfs
        .read_dir(&PathBuf::from("/r"))
        .expect("read_dir should succeed");
    assert_eq!(
        entries,
        vec![
            DirEntry {
                name: "apple".to_string(),
                path: PathBuf::from("/r/apple"),
                is_dir: false
            },
            DirEntry {
                name: "Banana".to_string(),
                path: PathBuf::from("/r/Banana"),
                is_dir: false
            },
            DirEntry {
                name: "File.md".to_string(),
                path: PathBuf::from("/r/File.md"),
                is_dir: false
            },
            DirEntry {
                name: "file.md".to_string(),
                path: PathBuf::from("/r/file.md"),
                is_dir: false
            },
        ],
        "case-insensitive primary order (apple before Banana), exact-name tiebreak (File.md before file.md)"
    );
}

/// A key equal to `path` itself (e.g. `path` was saved as a file) is not a
/// child of itself and must not appear in its own listing.
#[test]
fn mem_read_dir_excludes_the_queried_path_itself() {
    let vfs = Mem::new();
    vfs.save_atomic(&PathBuf::from("/a"), b"a-is-a-file")
        .expect("save /a as a file");
    vfs.save_atomic(&PathBuf::from("/a/b.md"), b"b")
        .expect("save /a/b.md");

    let entries = vfs
        .read_dir(&PathBuf::from("/a"))
        .expect("read_dir should succeed");
    assert_eq!(
        entries,
        vec![DirEntry {
            name: "b.md".to_string(),
            path: PathBuf::from("/a/b.md"),
            is_dir: false
        }],
        "the key `/a` itself must not appear as a child of `/a`"
    );
}

/// The collision the previous test's setup also creates, one level up:
/// `/a` exists both as a FILE key and as a directory PREFIX (`/a/b.md`
/// makes `a` a synthetic directory under `/`). Listing `/a`'s own children
/// correctly excludes `/a` itself (the test above) — but listing `/a`'s
/// PARENT must show `a` exactly ONCE, as a directory, never twice (once
/// `is_dir: false` from the `/a` key, once `is_dir: true` from the
/// `/a/b.md` key) — the bug this fix removes.
#[test]
fn mem_read_dir_parent_listing_dedups_a_name_claimed_as_both_file_and_dir() {
    let vfs = Mem::new();
    vfs.save_atomic(&PathBuf::from("/a"), b"a-is-a-file")
        .expect("save /a as a file");
    vfs.save_atomic(&PathBuf::from("/a/b.md"), b"b")
        .expect("save /a/b.md");

    let entries = vfs
        .read_dir(&PathBuf::from("/"))
        .expect("read_dir should succeed");
    assert_eq!(
        entries,
        vec![DirEntry {
            name: "a".to_string(),
            path: PathBuf::from("/a"),
            is_dir: true
        }],
        "`a` must appear exactly once, as a directory — the directory claim wins"
    );
}

/// WP13.S1 (finding `rune-tui C 1`): a name that isn't valid UTF-8 gets a
/// lossy `name` (display-only, may contain U+FFFD) but `path` must still
/// be the byte-exact key — round-tripping it back through `Vfs::stat`
/// (rather than rebuilding a path from `name`, which is exactly the
/// mangling this field exists to make unrepresentable) must find the same
/// file.
#[test]
fn mem_read_dir_path_is_byte_exact_for_a_non_utf8_name() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let vfs = Mem::new();
    let raw_name = OsStr::from_bytes(b"caf\xE9.md"); // invalid UTF-8 (Latin-1 'é')
    let raw_path = PathBuf::from("/").join(raw_name);
    vfs.save_atomic(&raw_path, b"content")
        .expect("save non-UTF-8 named file");

    let entries = vfs
        .read_dir(&PathBuf::from("/"))
        .expect("read_dir should succeed");
    assert_eq!(entries.len(), 1);
    let entry = entries.first().expect("exactly one entry");
    assert!(
        entry.name.contains('\u{FFFD}'),
        "the lossy display name must show the replacement character, got {:?}",
        entry.name
    );
    assert_eq!(
        entry.path, raw_path,
        "`path` must be the byte-exact key, never rebuilt from the lossy `name`"
    );
    assert_eq!(
        vfs.read(&entry.path).expect("read via the exact path"),
        b"content"
    );
}
