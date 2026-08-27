//! `Disk::write_durable`'s temp file mode: opened private (`0o600`) so the
//! plaintext is never briefly world-readable, and — for an overwrite —
//! ending up at the destination's own existing mode once the publish swaps
//! it in.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rune_vfs::{Disk, Vfs, VfsTestExt};

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "rune-vfs-write-durable-perms-{label}-{}",
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

fn mode_of(path: &Path) -> u32 {
    fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

#[test]
fn write_durable_of_a_fresh_destination_opens_the_temp_at_mode_0600() {
    let scratch = Scratch::new("fresh");
    let path = scratch.path().join("new.md");

    let temp = Disk.write_durable(&path, b"hello").expect("write_durable");

    assert_eq!(mode_of(&temp), 0o600);
    let _ = fs::remove_file(&temp);
}

#[test]
fn write_durable_over_an_existing_destination_copies_its_mode_onto_the_temp() {
    let scratch = Scratch::new("overwrite");
    let path = scratch.path().join("doc.md");
    fs::write(&path, b"original").expect("seed destination");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).expect("chmod destination");

    let temp = Disk
        .write_durable(&path, b"updated")
        .expect("write_durable");

    assert_eq!(
        mode_of(&temp),
        0o640,
        "the temp must carry the destination's own mode, not the 0o600 it was opened at"
    );
    let _ = fs::remove_file(&temp);
}

#[test]
fn save_atomic_of_a_brand_new_document_ends_up_private_at_mode_0600() {
    let scratch = Scratch::new("save-fresh");
    let path = scratch.path().join("new.md");

    Disk.save_atomic(&path, b"hello").expect("save_atomic");

    assert_eq!(mode_of(&path), 0o600);
}

#[test]
fn save_atomic_over_an_existing_document_preserves_its_mode_through_the_swap() {
    let scratch = Scratch::new("save-overwrite");
    let path = scratch.path().join("doc.md");
    fs::write(&path, b"original").expect("seed destination");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod destination");

    Disk.save_atomic(&path, b"updated").expect("save_atomic");

    assert_eq!(
        mode_of(&path),
        0o644,
        "an overwrite must never silently change a pre-existing file's mode"
    );
}
