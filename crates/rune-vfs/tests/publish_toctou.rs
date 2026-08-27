//! Regression tests for two `publish.rs` defects, both against the real
//! `Disk` backend:
//!
//! - `put_force` must refuse to exchange a regular file with a directory
//!   (the destination-kind gate `put_if_match` already gets for free through
//!   `get_resolved`, but `put_force` did not).
//! - `put_if_match`'s CAS check and its publish must land on the exact same
//!   resolution of the target path, never two independent `Vfs::resolve`
//!   calls a symlink re-point could straddle.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use rune_vfs::{DirEntry, Disk, PutCondition, PutOutcome, Stat, Vfs, etag_of, put};

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "rune-vfs-publish-toctou-{label}-{}",
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
fn disk_force_over_a_directory_refuses_and_leaves_it_intact() {
    let scratch = Scratch::new("dir-swap");
    let root = scratch.path();
    let notes = root.join("notes");
    fs::create_dir(&notes).expect("mkdir notes");
    fs::write(notes.join("a.md"), b"content").expect("seed file");

    let err = put(
        &Disk,
        &notes,
        b"anything",
        PutCondition::Force { expect: None },
    )
    .unwrap_err();

    assert_eq!(err.kind(), io::ErrorKind::IsADirectory);
    assert!(notes.is_dir(), "notes/ must still be a directory");
    assert_eq!(
        fs::read(notes.join("a.md")).expect("original file preserved"),
        b"content"
    );
    let stray: Vec<_> = fs::read_dir(root)
        .expect("read root")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .filter(|name| name.to_string_lossy().contains("rune-tmp"))
        .collect();
    assert!(
        stray.is_empty(),
        "no stray temp must remain next to notes/: {stray:?}"
    );
}

struct RepointSymlinkAfterFirstResolve {
    link: PathBuf,
    victim_target: PathBuf,
    calls: AtomicU32,
}

impl Vfs for RepointSymlinkAfterFirstResolve {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        Disk.read(path)
    }
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        Disk.write_durable(path, bytes)
    }
    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        Disk.exchange(a, b)
    }
    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()> {
        Disk.rename_excl(old, new)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        Disk.remove(path)
    }
    fn trash(&self, path: &Path) -> io::Result<()> {
        Disk.trash(path)
    }
    fn stat(&self, path: &Path) -> io::Result<Stat> {
        Disk.stat(path)
    }
    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        Disk.read_link(path)
    }
    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let result = Disk.resolve(path);
        if n == 1 {
            fs::remove_file(&self.link).expect("remove old symlink");
            std::os::unix::fs::symlink(&self.victim_target, &self.link)
                .expect("repoint symlink to the victim");
        }
        result
    }
    fn mkdir_all(&self, path: &Path) -> io::Result<()> {
        Disk.mkdir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        Disk.read_dir(path)
    }
}

#[test]
fn disk_put_if_match_publishes_over_the_same_resolution_the_etag_was_checked_against() {
    let scratch = Scratch::new("symlink-swap");
    let root = scratch.path();
    let real = root.join("real.md");
    let victim = root.join("victim.md");
    let link = root.join("doc.md");
    fs::write(&real, b"real content").expect("seed real");
    fs::write(&victim, b"victim content").expect("seed victim");
    std::os::unix::fs::symlink(&real, &link).expect("seed symlink to real");

    let vfs = RepointSymlinkAfterFirstResolve {
        link: link.clone(),
        victim_target: victim.clone(),
        calls: AtomicU32::new(0),
    };

    let outcome = put(
        &vfs,
        &link,
        b"attacker bytes",
        PutCondition::IfMatch(etag_of(b"real content")),
    )
    .unwrap();

    assert!(
        matches!(outcome, PutOutcome::Committed { .. }),
        "expected a commit, got {outcome:?}"
    );
    assert_eq!(
        fs::read(&real).expect("real.md"),
        b"attacker bytes",
        "the CAS'd write must land on real.md, the file the etag was checked against"
    );
    assert_eq!(
        fs::read(&victim).expect("victim.md"),
        b"victim content",
        "victim.md, only linked in after the check, must be untouched"
    );
}
