#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::Mem;

fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

#[test]
fn get_on_a_quiescent_file_is_confirmed() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"hello");

    let sighting = get(&vfs, path, MAX_DOCUMENT_BYTES).expect("get");
    assert!(sighting.sighted.is_confirmed());
    assert_eq!(sighting.bytes, b"hello");
    assert_eq!(sighting.etag, etag_of(b"hello"));
}

#[test]
fn get_of_a_missing_path_refuses_not_found() {
    let vfs = Mem::new();
    let err = get(&vfs, Path::new("/gone.md"), MAX_DOCUMENT_BYTES).expect_err("must refuse");
    assert!(matches!(err, GetRefusal::NotFound));
}

#[test]
fn get_of_a_directory_refuses_not_a_file() {
    let vfs = Mem::new();
    publish(&vfs, Path::new("/a/b.md"), b"content");

    let err = get(&vfs, Path::new("/a"), MAX_DOCUMENT_BYTES).expect_err("must refuse");
    assert!(matches!(err, GetRefusal::NotAFile(FileKind::Dir)));
}

#[test]
fn get_of_a_fifo_refuses_not_a_file() {
    let vfs = Mem::new();
    publish(&vfs, Path::new("/fifo"), b"");
    vfs.set_kind(Path::new("/fifo"), FileKind::Other)
        .expect("set_kind");

    let err = get(&vfs, Path::new("/fifo"), MAX_DOCUMENT_BYTES).expect_err("must refuse");
    assert!(matches!(err, GetRefusal::NotAFile(FileKind::Other)));
}

#[test]
fn get_over_max_bytes_refuses_too_large() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"0123456789");

    let err = get(&vfs, path, 5).expect_err("must refuse");
    assert!(matches!(err, GetRefusal::TooLarge { size: 10, limit: 5 }));
}

#[test]
fn get_unconfirmed_after_exhausting_retries_still_returns_bytes() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"before");
    vfs.set_churning(path, true);

    let sighting = get(&vfs, path, MAX_DOCUMENT_BYTES).expect("get");
    assert!(!sighting.sighted.is_confirmed());
}

struct FailOneStatCallVfs {
    inner: Mem,
    calls: std::sync::atomic::AtomicU32,
    fail_on_call: u32,
}

impl Vfs for FailOneStatCallVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<std::path::PathBuf> {
        self.inner.write_durable(path, bytes)
    }
    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        self.inner.exchange(a, b)
    }
    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()> {
        self.inner.rename_excl(old, new)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.inner.remove(path)
    }
    fn trash(&self, path: &Path) -> io::Result<()> {
        self.inner.trash(path)
    }
    fn stat(&self, path: &Path) -> io::Result<Stat> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        if call == self.fail_on_call {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "transient stat failure",
            ));
        }
        self.inner.stat(path)
    }
    fn resolve(&self, path: &Path) -> io::Result<std::path::PathBuf> {
        self.inner.resolve(path)
    }
    fn mkdir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.mkdir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<crate::DirEntry>> {
        self.inner.read_dir(path)
    }
    fn read_link(&self, path: &Path) -> io::Result<std::path::PathBuf> {
        self.inner.read_link(path)
    }
}

#[test]
fn get_recovers_from_a_transient_stat_failure_inside_the_bracket() {
    let inner = Mem::new();
    let path = Path::new("/doc.md");
    publish(&inner, path, b"hello");
    // Call 1 is `get`'s own FileKind/size gate stat; call 2 is the
    // bracket's first "before" stat, which is the one this test makes
    // transiently fail.
    let vfs = FailOneStatCallVfs {
        inner,
        calls: std::sync::atomic::AtomicU32::new(0),
        fail_on_call: 2,
    };

    let sighting = get(&vfs, path, MAX_DOCUMENT_BYTES).expect("get");
    assert!(
        sighting.sighted.is_confirmed(),
        "the bracket's retry must recover once the transient failure is consumed"
    );
}

#[test]
fn get_settles_on_a_mutation_between_the_two_stats() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    publish(&vfs, path, b"hello");
    vfs.mutate_after_next_stat(path, b"after".to_vec());

    let sighting = get(&vfs, path, MAX_DOCUMENT_BYTES).expect("get");
    assert!(sighting.sighted.is_confirmed());
    assert_eq!(sighting.bytes, b"after");
}

struct FailStatAfterFirstVfs {
    inner: Mem,
    stat_calls: std::sync::atomic::AtomicU32,
}

impl Vfs for FailStatAfterFirstVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<std::path::PathBuf> {
        self.inner.write_durable(path, bytes)
    }
    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        self.inner.exchange(a, b)
    }
    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()> {
        self.inner.rename_excl(old, new)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.inner.remove(path)
    }
    fn trash(&self, path: &Path) -> io::Result<()> {
        self.inner.trash(path)
    }
    fn stat(&self, path: &Path) -> io::Result<Stat> {
        let call = self
            .stat_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if call == 0 {
            return self.inner.stat(path);
        }
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "stat always fails after the first call",
        ))
    }
    fn resolve(&self, path: &Path) -> io::Result<std::path::PathBuf> {
        self.inner.resolve(path)
    }
    fn mkdir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.mkdir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<crate::DirEntry>> {
        self.inner.read_dir(path)
    }
    fn read_link(&self, path: &Path) -> io::Result<std::path::PathBuf> {
        self.inner.read_link(path)
    }
}

#[test]
fn max_document_bytes_is_exactly_64_mebibytes() {
    assert_eq!(MAX_DOCUMENT_BYTES, 67_108_864);
}

#[test]
fn sighted_stat_returns_the_confirmed_value() {
    let stat = Stat {
        size: 5,
        mtime: std::time::UNIX_EPOCH,
        identity: crate::Identity {
            inode: Some(1),
            device: Some(2),
        },
        nlink: Some(1),
        kind: FileKind::File,
    };
    assert_eq!(Sighted::Confirmed(stat).stat(), Some(stat));
}

#[test]
fn get_refusal_display_matches_the_expected_message_per_variant() {
    assert_eq!(GetRefusal::NotFound.to_string(), "not found");
    assert_eq!(
        GetRefusal::NotAFile(FileKind::Dir).to_string(),
        "not a regular file (Dir)"
    );
    assert_eq!(
        GetRefusal::TooLarge { size: 10, limit: 5 }.to_string(),
        "too large (10 bytes; limit 5)"
    );
}

struct PartialIdentityVfs {
    inner: Mem,
}

impl Vfs for PartialIdentityVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<std::path::PathBuf> {
        self.inner.write_durable(path, bytes)
    }
    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        self.inner.exchange(a, b)
    }
    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()> {
        self.inner.rename_excl(old, new)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.inner.remove(path)
    }
    fn trash(&self, path: &Path) -> io::Result<()> {
        self.inner.trash(path)
    }
    fn stat(&self, path: &Path) -> io::Result<Stat> {
        let mut stat = self.inner.stat(path)?;
        stat.identity.device = None;
        Ok(stat)
    }
    fn resolve(&self, path: &Path) -> io::Result<std::path::PathBuf> {
        self.inner.resolve(path)
    }
    fn mkdir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.mkdir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<crate::DirEntry>> {
        self.inner.read_dir(path)
    }
    fn read_link(&self, path: &Path) -> io::Result<std::path::PathBuf> {
        self.inner.read_link(path)
    }
}

#[test]
fn a_stat_missing_half_its_identity_is_never_treated_as_confirmed() {
    let inner = Mem::new();
    publish(&inner, Path::new("/doc.md"), b"hello");
    let vfs = PartialIdentityVfs { inner };

    let sighting = get(&vfs, Path::new("/doc.md"), MAX_DOCUMENT_BYTES).expect("get");
    assert!(
        !sighting.sighted.is_confirmed(),
        "an identity missing its device half must never confirm a sighting, \
         even when the same partial identity is reported on every stat"
    );
}

struct ReadCountingVfs {
    inner: Mem,
    read_calls: std::sync::atomic::AtomicU32,
}

impl Vfs for ReadCountingVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.read_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.read(path)
    }
    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<std::path::PathBuf> {
        self.inner.write_durable(path, bytes)
    }
    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        self.inner.exchange(a, b)
    }
    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()> {
        self.inner.rename_excl(old, new)
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.inner.remove(path)
    }
    fn trash(&self, path: &Path) -> io::Result<()> {
        self.inner.trash(path)
    }
    fn stat(&self, path: &Path) -> io::Result<Stat> {
        self.inner.stat(path)
    }
    fn resolve(&self, path: &Path) -> io::Result<std::path::PathBuf> {
        self.inner.resolve(path)
    }
    fn mkdir_all(&self, path: &Path) -> io::Result<()> {
        self.inner.mkdir_all(path)
    }
    fn read_dir(&self, path: &Path) -> io::Result<Vec<crate::DirEntry>> {
        self.inner.read_dir(path)
    }
    fn read_link(&self, path: &Path) -> io::Result<std::path::PathBuf> {
        self.inner.read_link(path)
    }
}

#[test]
fn bracketed_get_stops_retrying_after_exactly_bracket_max_attempts() {
    let inner = Mem::new();
    let path = Path::new("/doc.md");
    publish(&inner, path, b"hello");
    inner.set_churning(path, true);
    let vfs = ReadCountingVfs {
        inner,
        read_calls: std::sync::atomic::AtomicU32::new(0),
    };

    let (_, sighted) = bracketed_get(&vfs, path).expect("bracketed_get");

    assert!(!sighted.is_confirmed());
    assert_eq!(
        vfs.read_calls.load(std::sync::atomic::Ordering::SeqCst),
        BRACKET_MAX_ATTEMPTS,
        "a perpetually churning file must be read exactly BRACKET_MAX_ATTEMPTS times, no more"
    );
}

#[test]
fn get_resolved_accepts_a_file_exactly_at_the_byte_limit() {
    let vfs = Mem::new();
    let path = Path::new("/doc.md");
    let bytes = vec![0u8; 10];
    publish(&vfs, path, &bytes);

    let sighting = get(&vfs, path, 10).expect("a file exactly at the limit must be accepted");
    assert_eq!(sighting.bytes.len(), 10);
}

#[test]
fn get_reports_no_stat_when_the_bracket_stat_never_recovers() {
    let inner = Mem::new();
    publish(&inner, Path::new("/doc.md"), b"hello");
    let vfs = FailStatAfterFirstVfs {
        inner,
        stat_calls: std::sync::atomic::AtomicU32::new(0),
    };

    let sighting = get(&vfs, Path::new("/doc.md"), MAX_DOCUMENT_BYTES).expect("get");
    assert!(!sighting.sighted.is_confirmed());
    assert!(sighting.sighted.stat().is_none());
}
