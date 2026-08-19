//! [rune-nav 9, 11]: coverage gaps the in-crate unit tests left open —
//! both test helpers there hardcode `anchor: None`, so dropping
//! `anchor.clone()` in either branch of `resolve_candidate` kept all its
//! prior tests green; and `FileKind::Other` (the FIFO/socket/device case
//! `is_regular`'s own doc comment cites) was never exercised because
//! `Mem` has no way to represent it. This file is the crate's `tests/`
//! surface against the `Vfs`-injected boundary (plan WP12.S6).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io;
use std::path::{Path, PathBuf};

use rune_nav::{Anchor, AnchorRole, Destination, Target};
use rune_vfs::{DirEntry, FileKind, Mem, Stat, Vfs, VfsTestExt};

const MD: &str = "md";

fn mem_with(paths: &[&str]) -> Mem {
    let vfs = Mem::new();
    for p in paths {
        vfs.save_atomic(&PathBuf::from(p), b"content")
            .expect("seed file");
    }
    vfs
}

fn named(name: &str) -> Anchor {
    Anchor::Named {
        role: AnchorRole::Heading,
        name: name.to_string(),
    }
}

#[test]
fn a_path_target_carries_a_named_anchor_through_to_the_destination() {
    let vfs = mem_with(&["/root/note.md"]);
    let target = Target::Path {
        path: "note.md".to_string(),
        anchor: Some(named("Setup")),
    };
    let dest = rune_nav::resolve(&vfs, &target, None, Path::new("/root"), MD);
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/root/note.md"),
            anchor: Some(named("Setup")),
        }
    );
}

#[test]
fn a_name_target_carries_a_named_anchor_through_to_the_destination() {
    let vfs = mem_with(&["/root/note.md"]);
    let target = Target::Name {
        name: "note".to_string(),
        anchor: Some(named("Setup")),
    };
    let dest = rune_nav::resolve(&vfs, &target, None, Path::new("/root"), MD);
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/root/note.md"),
            anchor: Some(named("Setup")),
        }
    );
}

#[test]
fn a_line_anchor_survives_resolution_with_its_number_intact() {
    let vfs = mem_with(&["/root/note.md"]);
    let target = Target::Path {
        path: "note.md".to_string(),
        anchor: Some(Anchor::Line(42)),
    };
    let dest = rune_nav::resolve(&vfs, &target, None, Path::new("/root"), MD);
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/root/note.md"),
            anchor: Some(Anchor::Line(42)),
        }
    );
}

#[test]
fn an_absolute_target_also_carries_its_anchor_through() {
    let vfs = mem_with(&["/elsewhere/note.md"]);
    let target = Target::Path {
        path: "/elsewhere/note.md".to_string(),
        anchor: Some(Anchor::Line(7)),
    };
    let dest = rune_nav::resolve(&vfs, &target, None, Path::new("/root"), MD);
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/elsewhere/note.md"),
            anchor: Some(Anchor::Line(7)),
        }
    );
}

/// A `Vfs` test double whose `stat` reports a fixed `FileKind` for every
/// path, regardless of what (if anything) was ever "saved" — the only way
/// to exercise `FileKind::Other` (a FIFO, socket, or device node), since
/// `Mem` has no representation for one.
struct FixedKindVfs(FileKind);

impl Vfs for FixedKindVfs {
    fn read(&self, _path: &Path) -> io::Result<Vec<u8>> {
        unimplemented!("not exercised by resolve()")
    }
    fn write_durable(&self, _path: &Path, _bytes: &[u8]) -> io::Result<PathBuf> {
        unimplemented!("not exercised by resolve()")
    }
    fn exchange(&self, _a: &Path, _b: &Path) -> io::Result<()> {
        unimplemented!("not exercised by resolve()")
    }
    fn rename_excl(&self, _old: &Path, _new: &Path) -> io::Result<()> {
        unimplemented!("not exercised by resolve()")
    }
    fn remove(&self, _path: &Path) -> io::Result<()> {
        unimplemented!("not exercised by resolve()")
    }
    fn trash(&self, _path: &Path) -> io::Result<()> {
        unimplemented!("not exercised by resolve()")
    }
    fn stat(&self, _path: &Path) -> io::Result<Stat> {
        Ok(Stat {
            size: 0,
            mtime: std::time::SystemTime::UNIX_EPOCH,
            identity: rune_vfs::Identity::default(),
            nlink: None,
            kind: self.0,
        })
    }
    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        Ok(path.to_path_buf())
    }
    fn mkdir_all(&self, _path: &Path) -> io::Result<()> {
        unimplemented!("not exercised by resolve()")
    }
    fn read_dir(&self, _path: &Path) -> io::Result<Vec<DirEntry>> {
        unimplemented!("not exercised by resolve()")
    }
    fn read_link(&self, _path: &Path) -> io::Result<PathBuf> {
        unimplemented!("not exercised by resolve()")
    }
}

#[test]
fn a_fifo_or_device_node_never_resolves_as_a_link_target() {
    let vfs = FixedKindVfs(FileKind::Other);
    let target = Target::Path {
        path: "/root/some-fifo".to_string(),
        anchor: None,
    };
    let dest = rune_nav::resolve(&vfs, &target, None, Path::new("/root"), MD);
    assert_eq!(dest, Destination::Unresolved);
}

#[test]
fn a_directory_reported_by_stat_never_resolves_as_a_link_target() {
    let vfs = FixedKindVfs(FileKind::Dir);
    let target = Target::Path {
        path: "/root/sub".to_string(),
        anchor: None,
    };
    let dest = rune_nav::resolve(&vfs, &target, None, Path::new("/root"), MD);
    assert_eq!(dest, Destination::Unresolved);
}

#[test]
fn a_name_target_with_an_extension_already_present_is_not_appended_twice() {
    let vfs = mem_with(&["/root/notes.txt"]);
    let target = Target::Name {
        name: "notes.txt".to_string(),
        anchor: None,
    };
    let dest = rune_nav::resolve(&vfs, &target, None, Path::new("/root"), MD);
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/root/notes.txt"),
            anchor: None,
        }
    );
}

#[test]
fn a_none_doc_dir_still_resolves_against_root() {
    let vfs = mem_with(&["/root/note.md"]);
    let target = Target::Path {
        path: "note.md".to_string(),
        anchor: None,
    };
    let dest = rune_nav::resolve(&vfs, &target, None, Path::new("/root"), MD);
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/root/note.md"),
            anchor: None,
        }
    );
}
