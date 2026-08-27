//! `Disk`'s durability-confirmation seam: the parent-directory fsync that
//! runs after every flagged rename (`Disk::publish`), the fsync target it
//! picks (`parent_to_fsync`), and the symlink-hop accounting `resolve_leaf`
//! uses to refuse a cyclic chain instead of recursing forever.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rune_vfs::{Disk, MAX_SYMLINK_HOPS, Vfs, published_not_durable};

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "rune-vfs-disk-durability-{label}-{}",
            std::process::id()
        ));
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

#[test]
fn a_publish_whose_destination_dir_cannot_be_reopened_reports_published_not_durable() {
    let scratch = Scratch::new("fsync-fail");
    let root = scratch.path();
    let locked = root.join("locked");
    fs::create_dir(&locked).expect("mkdir locked");
    let dest = locked.join("doc.md");

    let temp = Disk.write_durable(&dest, b"hello").expect("write_durable");
    // Write+execute (needed to rename within the dir) but NOT read (needed to
    // reopen the dir for the post-rename fsync): the rename itself must still
    // succeed, but the durability confirmation that follows it must fail.
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o300)).expect("chmod locked");

    let result = Disk.rename_excl(&temp, &dest);

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("restore perms");

    let err = result.expect_err("the parent fsync must fail when the dir can't be reopened");
    assert!(
        published_not_durable(&err),
        "the rename already took effect; this must be marked published_not_durable"
    );
    assert_eq!(
        fs::read(&dest).expect("the rename physically succeeded"),
        b"hello"
    );
}

#[test]
fn rename_excl_onto_a_bare_relative_filename_fsyncs_the_cwd_not_an_empty_path() {
    let scratch = Scratch::new("bare-relative");
    let old_cwd = std::env::current_dir().expect("get cwd");
    std::env::set_current_dir(scratch.path()).expect("chdir to scratch");

    let result = (|| -> std::io::Result<()> {
        let temp = Disk.write_durable(Path::new("src.md"), b"content")?;
        Disk.rename_excl(&temp, Path::new("dest.md"))
    })();

    std::env::set_current_dir(&old_cwd).expect("restore cwd");

    result.expect(
        "a bare relative destination has an empty `parent()`; the fsync target must fall back \
         to \".\", never try to open the empty path itself",
    );
    assert_eq!(
        fs::read(scratch.path().join("dest.md")).expect("read dest"),
        b"content"
    );
}

#[test]
fn rename_excl_onto_a_real_nonempty_parent_fsyncs_that_parent_not_the_cwd() {
    let scratch = Scratch::new("real-parent");
    let root = scratch.path();
    let locked = root.join("locked");
    fs::create_dir(&locked).expect("mkdir locked");
    let dest = locked.join("doc.md");
    let temp = Disk.write_durable(&dest, b"hello").expect("write_durable");

    // The CWD (unrelated to `locked`) is left fully accessible. If
    // `parent_to_fsync` mistakenly fsyncs the CWD instead of `locked`, this
    // publish would wrongly report success.
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o300)).expect("chmod locked");
    let result = Disk.rename_excl(&temp, &dest);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("restore perms");

    let err = result.expect_err(
        "a nonempty, real parent directory must be the fsync target — \
         fsyncing anything else would hide this dir's unreadability",
    );
    assert!(published_not_durable(&err));
}

#[test]
fn resolve_refuses_a_self_referential_symlink_instead_of_recursing_forever() {
    let scratch = Scratch::new("self-loop");
    let link = scratch.path().join("a");
    std::os::unix::fs::symlink("a", &link).expect("create self-referential symlink");

    let err = Disk
        .resolve(&link)
        .expect_err("a self-referential symlink must refuse, not hang");

    assert_eq!(err.raw_os_error(), Some(libc::ELOOP));
}

fn seed_disk_chain(root: &Path, hops: usize) {
    fs::write(root.join("real.md"), b"real").expect("write real.md");
    for hop in 0..hops {
        let target = if hop + 1 == hops {
            "real.md".to_string()
        } else {
            format!("hop{}.md", hop + 1)
        };
        std::os::unix::fs::symlink(target, root.join(format!("hop{hop}.md")))
            .expect("create chain link");
    }
}

#[test]
fn resolve_follows_a_symlink_chain_exactly_at_the_hop_limit() {
    let scratch = Scratch::new("chain-at-limit");
    let root = scratch.path();
    seed_disk_chain(root, MAX_SYMLINK_HOPS);

    let resolved = Disk
        .resolve(&root.join("hop0.md"))
        .expect("a chain of exactly MAX_SYMLINK_HOPS links must still resolve");

    assert_eq!(
        resolved,
        fs::canonicalize(root.join("real.md")).expect("canonicalize real.md")
    );
}

#[test]
fn resolve_refuses_a_symlink_chain_one_past_the_hop_limit() {
    let scratch = Scratch::new("chain-past-limit");
    let root = scratch.path();
    seed_disk_chain(root, MAX_SYMLINK_HOPS + 1);

    let err = Disk
        .resolve(&root.join("hop0.md"))
        .expect_err("a chain one hop past the limit must refuse");

    assert_eq!(err.raw_os_error(), Some(libc::ELOOP));
}

#[test]
fn resolving_a_new_files_leaf_through_a_symlinked_parent_lands_on_the_real_directory() {
    let scratch = Scratch::new("new-leaf-via-symlinked-parent");
    let root = scratch.path();
    let real_dir = root.join("real_dir");
    fs::create_dir(&real_dir).expect("mkdir real_dir");
    let alias_dir = root.join("alias_dir");
    std::os::unix::fs::symlink(&real_dir, &alias_dir).expect("create alias_dir symlink");

    let resolved = Disk
        .resolve(&alias_dir.join("new_file.md"))
        .expect("resolve a not-yet-existing leaf through a symlinked parent");

    assert_eq!(
        resolved,
        fs::canonicalize(&real_dir)
            .expect("canonicalize real_dir")
            .join("new_file.md"),
        "the leaf's parent must be canonicalized through the symlink, landing on real_dir"
    );
}

#[test]
fn mkdir_all_actually_creates_the_directory_tree() {
    let scratch = Scratch::new("mkdir-all");
    let nested = scratch.path().join("a").join("b").join("c");

    Disk.mkdir_all(&nested).expect("mkdir_all");

    assert!(nested.is_dir(), "the full nested path must exist on disk");
}

#[test]
fn trash_actually_removes_the_file_from_its_original_location() {
    let scratch = Scratch::new("trash");
    let path = scratch.path().join("doc.md");
    fs::write(&path, b"trash me").expect("seed file");

    Disk.trash(&path).expect("trash");

    assert!(
        !path.exists(),
        "trash must physically remove the file from its original path"
    );
}

#[test]
fn rename_excl_over_an_existing_destination_refuses_instead_of_clobbering() {
    let scratch = Scratch::new("excl-no-clobber");
    let root = scratch.path();
    let dest = root.join("dest.md");
    fs::write(&dest, b"already here").expect("seed dest");

    let temp = Disk
        .write_durable(&dest, b"new content")
        .expect("write_durable");
    let err = Disk
        .rename_excl(&temp, &dest)
        .expect_err("rename_excl over an existing destination must refuse, never clobber");

    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        fs::read(&dest).expect("dest untouched"),
        b"already here",
        "the excl flag must have prevented the clobber; the original content survives"
    );
    let _ = fs::remove_file(&temp);
}
