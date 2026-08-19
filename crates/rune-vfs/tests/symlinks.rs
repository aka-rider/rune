//! Symlink behavior, for both `Disk` and `Mem`: what `read_dir` reports for a
//! link, what `read_link` gives back, and how far `resolve` follows.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rune_vfs::{DirEntry, Disk, FileKind, Link, MAX_SYMLINK_HOPS, Mem, Vfs, VfsTestExt};
use std::fs;
use std::path::{Path, PathBuf};

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let dir =
            std::env::temp_dir().join(format!("rune-vfs-symlinks-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        Scratch(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn entry<'a>(entries: &'a [DirEntry], name: &str) -> &'a DirEntry {
    entries
        .iter()
        .find(|e| e.name == name)
        .expect("the listing must contain the named entry")
}

fn names(entries: &[DirEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}

#[test]
fn disk_lists_a_symlink_to_a_directory_as_a_directory_sorted_among_the_directories() {
    let scratch = Scratch::new("disk-dirlink");
    let root = scratch.path();
    fs::create_dir(root.join("real")).expect("mkdir real");
    fs::write(root.join("zeta.md"), b"z").expect("write zeta");
    std::os::unix::fs::symlink(root.join("real"), root.join("alias")).expect("create symlink");

    let entries = Disk.read_dir(root).expect("read_dir");

    assert_eq!(entry(&entries, "alias").kind, FileKind::Dir);
    assert_eq!(entry(&entries, "alias").link, Link::To);
    assert_eq!(names(&entries), vec!["alias", "real", "zeta.md"]);
}

#[test]
fn mem_lists_a_symlink_to_a_directory_as_a_directory_sorted_among_the_directories() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/root/real/inner.md"), b"i")
        .expect("seed nested file");
    vfs.save_atomic(Path::new("/root/zeta.md"), b"z")
        .expect("seed file");
    vfs.symlink(Path::new("/root/alias"), Path::new("/root/real"))
        .expect("seed symlink");

    let entries = vfs.read_dir(Path::new("/root")).expect("read_dir");

    assert_eq!(entry(&entries, "alias").kind, FileKind::Dir);
    assert_eq!(entry(&entries, "alias").link, Link::To);
    assert_eq!(names(&entries), vec!["alias", "real", "zeta.md"]);
}

#[test]
fn disk_lists_a_symlink_to_a_file_as_a_file() {
    let scratch = Scratch::new("disk-filelink");
    let root = scratch.path();
    fs::write(root.join("real.md"), b"r").expect("write real");
    std::os::unix::fs::symlink(root.join("real.md"), root.join("alias.md"))
        .expect("create symlink");

    let entries = Disk.read_dir(root).expect("read_dir");

    assert_eq!(entry(&entries, "alias.md").kind, FileKind::File);
    assert_eq!(entry(&entries, "alias.md").link, Link::To);
    assert_eq!(entry(&entries, "real.md").link, Link::No);
}

#[test]
fn mem_lists_a_symlink_to_a_file_as_a_file() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/root/real.md"), b"r")
        .expect("seed file");
    vfs.symlink(Path::new("/root/alias.md"), Path::new("/root/real.md"))
        .expect("seed symlink");

    let entries = vfs.read_dir(Path::new("/root")).expect("read_dir");

    assert_eq!(entry(&entries, "alias.md").kind, FileKind::File);
    assert_eq!(entry(&entries, "alias.md").link, Link::To);
    assert_eq!(entry(&entries, "real.md").link, Link::No);
}

#[test]
fn disk_lists_a_dangling_symlink_as_broken_with_no_kind_of_its_own() {
    let scratch = Scratch::new("disk-dangling");
    let root = scratch.path();
    std::os::unix::fs::symlink(root.join("gone.md"), root.join("alias.md"))
        .expect("create symlink");

    let entries = Disk.read_dir(root).expect("read_dir");

    assert_eq!(entry(&entries, "alias.md").link, Link::Broken);
    assert_eq!(entry(&entries, "alias.md").kind, FileKind::Other);
}

#[test]
fn mem_lists_a_dangling_symlink_as_broken_with_no_kind_of_its_own() {
    let vfs = Mem::new();
    vfs.symlink(Path::new("/root/alias.md"), Path::new("/root/gone.md"))
        .expect("seed symlink");

    let entries = vfs.read_dir(Path::new("/root")).expect("read_dir");

    assert_eq!(entry(&entries, "alias.md").link, Link::Broken);
    assert_eq!(entry(&entries, "alias.md").kind, FileKind::Other);
}

#[test]
fn disk_lists_a_self_referential_symlink_as_broken_instead_of_hanging() {
    let scratch = Scratch::new("disk-selfloop");
    let root = scratch.path();
    std::os::unix::fs::symlink("a", root.join("a")).expect("create symlink");

    let entries = Disk.read_dir(root).expect("read_dir");

    assert_eq!(entry(&entries, "a").link, Link::Broken);
    assert_eq!(entry(&entries, "a").kind, FileKind::Other);
}

#[test]
fn mem_lists_a_self_referential_symlink_as_broken_instead_of_hanging() {
    let vfs = Mem::new();
    vfs.symlink(Path::new("/root/a"), Path::new("a"))
        .expect("seed symlink");

    let entries = vfs.read_dir(Path::new("/root")).expect("read_dir");

    assert_eq!(entry(&entries, "a").link, Link::Broken);
    assert_eq!(entry(&entries, "a").kind, FileKind::Other);
}

#[test]
fn disk_resolves_a_relative_symlink_target_against_the_links_own_parent() {
    let scratch = Scratch::new("disk-relative");
    let root = scratch.path();
    fs::create_dir(root.join("sub")).expect("mkdir sub");
    fs::write(root.join("sub").join("real.md"), b"r").expect("write real");
    std::os::unix::fs::symlink("real.md", root.join("sub").join("alias.md"))
        .expect("create symlink");

    let entries = Disk.read_dir(&root.join("sub")).expect("read_dir");

    assert_eq!(entry(&entries, "alias.md").kind, FileKind::File);
    assert_eq!(entry(&entries, "alias.md").link, Link::To);
    assert_eq!(
        Disk.resolve(&root.join("sub").join("alias.md"))
            .expect("resolve"),
        fs::canonicalize(root.join("sub").join("real.md")).expect("canonicalize")
    );
}

#[test]
fn mem_resolves_a_relative_symlink_target_against_the_links_own_parent() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/root/sub/real.md"), b"r")
        .expect("seed file");
    vfs.symlink(Path::new("/root/sub/alias.md"), Path::new("real.md"))
        .expect("seed symlink");

    let entries = vfs.read_dir(Path::new("/root/sub")).expect("read_dir");

    assert_eq!(entry(&entries, "alias.md").kind, FileKind::File);
    assert_eq!(entry(&entries, "alias.md").link, Link::To);
    assert_eq!(
        vfs.resolve(Path::new("/root/sub/alias.md"))
            .expect("resolve"),
        PathBuf::from("/root/sub/real.md")
    );
}

#[test]
fn read_link_gives_back_the_target_exactly_as_it_was_written() {
    let scratch = Scratch::new("readlink-target");
    let root = scratch.path();
    fs::write(root.join("real.md"), b"r").expect("write real");
    std::os::unix::fs::symlink("real.md", root.join("alias.md")).expect("create symlink");

    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/root/real.md"), b"r")
        .expect("seed file");
    vfs.symlink(Path::new("/root/alias.md"), Path::new("real.md"))
        .expect("seed symlink");

    assert_eq!(
        Disk.read_link(&root.join("alias.md")).expect("read_link"),
        PathBuf::from("real.md")
    );
    assert_eq!(
        vfs.read_link(Path::new("/root/alias.md"))
            .expect("read_link"),
        PathBuf::from("real.md")
    );
}

#[test]
fn read_link_refuses_a_path_that_is_not_a_symlink_the_same_way_on_both_backends() {
    let scratch = Scratch::new("readlink-plain");
    let root = scratch.path();
    fs::write(root.join("real.md"), b"r").expect("write real");

    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/root/real.md"), b"r")
        .expect("seed file");

    let disk_err = Disk
        .read_link(&root.join("real.md"))
        .expect_err("a plain file is not a symlink");
    let mem_err = vfs
        .read_link(Path::new("/root/real.md"))
        .expect_err("a plain file is not a symlink");

    assert_eq!(disk_err.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(mem_err.kind(), disk_err.kind());
}

#[test]
fn read_link_reports_a_missing_path_as_not_found_on_both_backends() {
    let scratch = Scratch::new("readlink-missing");

    let disk_err = Disk
        .read_link(&scratch.path().join("gone.md"))
        .expect_err("nothing is there");
    let mem_err = Mem::new()
        .read_link(Path::new("/root/gone.md"))
        .expect_err("nothing is there");

    assert_eq!(disk_err.kind(), std::io::ErrorKind::NotFound);
    assert_eq!(mem_err.kind(), disk_err.kind());
}

#[test]
fn mem_follows_a_symlink_chain_up_to_the_hop_limit() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/root/real.md"), b"r")
        .expect("seed file");
    seed_chain(&vfs, MAX_SYMLINK_HOPS);

    assert_eq!(
        vfs.resolve(Path::new("/root/hop0.md")).expect("resolve"),
        PathBuf::from("/root/real.md")
    );
    assert_eq!(
        vfs.stat(Path::new("/root/hop0.md")).expect("stat").kind,
        FileKind::File
    );
}

#[test]
fn mem_refuses_a_symlink_chain_longer_than_the_hop_limit() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/root/real.md"), b"r")
        .expect("seed file");
    seed_chain(&vfs, MAX_SYMLINK_HOPS + 1);

    let err = vfs
        .resolve(Path::new("/root/hop0.md"))
        .expect_err("a chain past the hop limit must not resolve");

    assert_eq!(err.raw_os_error(), Some(libc::ELOOP));
}

fn seed_chain(vfs: &Mem, links: usize) {
    for hop in 0..links {
        let target = if hop + 1 == links {
            PathBuf::from("/root/real.md")
        } else {
            PathBuf::from(format!("/root/hop{}.md", hop + 1))
        };
        vfs.symlink(&PathBuf::from(format!("/root/hop{hop}.md")), &target)
            .expect("seed chain link");
    }
}

#[test]
fn mem_stats_a_symlink_as_the_file_it_points_at() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/root/real.md"), b"twelve bytes")
        .expect("seed file");
    vfs.symlink(Path::new("/root/alias.md"), Path::new("/root/real.md"))
        .expect("seed symlink");

    let through_link = vfs.stat(Path::new("/root/alias.md")).expect("stat");
    let direct = vfs.stat(Path::new("/root/real.md")).expect("stat");

    assert_eq!(through_link, direct);
}

#[test]
fn mem_saving_through_a_symlink_writes_the_target_and_leaves_the_link_a_link() {
    let vfs = Mem::new();
    vfs.save_atomic(Path::new("/root/real.md"), b"original")
        .expect("seed file");
    vfs.symlink(Path::new("/root/alias.md"), Path::new("/root/real.md"))
        .expect("seed symlink");

    vfs.save_atomic(Path::new("/root/alias.md"), b"new content")
        .expect("save through the symlink");

    assert_eq!(
        vfs.read(Path::new("/root/real.md")).expect("read target"),
        b"new content"
    );
    assert_eq!(
        vfs.read_link(Path::new("/root/alias.md"))
            .expect("the link must still be a link"),
        PathBuf::from("/root/real.md")
    );
}
