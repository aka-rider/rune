use std::io;
use std::path::Path;

use crate::{Etag, FileKind, Stat, Vfs, etag_of};

const BRACKET_MAX_ATTEMPTS: u32 = 3;

pub const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct Sighting {
    pub bytes: Vec<u8>,
    pub etag: Etag,
    pub stat: Option<Stat>,
    pub confirmed: bool,
}

#[derive(Debug)]
pub enum GetRefusal {
    NotFound,
    NotAFile(FileKind),
    TooLarge { size: u64, limit: u64 },
    Io(io::Error),
}

impl std::fmt::Display for GetRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GetRefusal::NotFound => f.write_str("not found"),
            GetRefusal::NotAFile(kind) => write!(f, "not a regular file ({kind:?})"),
            GetRefusal::TooLarge { size, limit } => {
                write!(f, "too large ({size} bytes; limit {limit})")
            }
            GetRefusal::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for GetRefusal {}

fn as_not_found(e: io::Error) -> GetRefusal {
    if e.kind() == io::ErrorKind::NotFound {
        GetRefusal::NotFound
    } else {
        GetRefusal::Io(e)
    }
}

fn stat_with_identity<V: Vfs + ?Sized>(vfs: &V, path: &Path) -> Option<Stat> {
    let stat = vfs.stat(path).ok()?;
    (stat.identity.inode.is_some() && stat.identity.device.is_some()).then_some(stat)
}

fn stat_matches(a: &Stat, b: &Stat) -> bool {
    a.size == b.size && a.mtime == b.mtime && a.identity == b.identity
}

fn one_get_bracket<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
) -> io::Result<(Vec<u8>, Option<Stat>, bool)> {
    let before = stat_with_identity(vfs, path);
    let bytes = vfs.read(path)?;
    let after = stat_with_identity(vfs, path);
    let confirmed = matches!((&before, &after), (Some(b), Some(a)) if stat_matches(b, a));
    let stat = after.or(before);
    Ok((bytes, stat, confirmed))
}

fn bracketed_get<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
) -> io::Result<(Vec<u8>, Option<Stat>, bool)> {
    let mut result = one_get_bracket(vfs, path)?;
    let mut attempts = 1;
    while !result.2 && attempts < BRACKET_MAX_ATTEMPTS {
        result = one_get_bracket(vfs, path)?;
        attempts += 1;
    }
    Ok(result)
}

pub fn get<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
    max_bytes: Option<u64>,
) -> Result<Sighting, GetRefusal> {
    let resolved = vfs.resolve(path).map_err(as_not_found)?;
    let stat = vfs.stat(&resolved).map_err(as_not_found)?;
    if stat.kind != FileKind::File {
        return Err(GetRefusal::NotAFile(stat.kind));
    }
    if let Some(limit) = max_bytes
        && stat.size > limit
    {
        return Err(GetRefusal::TooLarge {
            size: stat.size,
            limit,
        });
    }
    let (bytes, stat, confirmed) = bracketed_get(vfs, &resolved).map_err(GetRefusal::Io)?;
    let etag = etag_of(&bytes);
    Ok(Sighting {
        bytes,
        etag,
        stat,
        confirmed,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
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

        let sighting = get(&vfs, path, None).expect("get");
        assert!(sighting.confirmed);
        assert_eq!(sighting.bytes, b"hello");
        assert_eq!(sighting.etag, etag_of(b"hello"));
    }

    #[test]
    fn get_of_a_missing_path_refuses_not_found() {
        let vfs = Mem::new();
        let err = get(&vfs, Path::new("/gone.md"), None).expect_err("must refuse");
        assert!(matches!(err, GetRefusal::NotFound));
    }

    #[test]
    fn get_of_a_directory_refuses_not_a_file() {
        let vfs = Mem::new();
        publish(&vfs, Path::new("/a/b.md"), b"content");

        let err = get(&vfs, Path::new("/a"), None).expect_err("must refuse");
        assert!(matches!(err, GetRefusal::NotAFile(FileKind::Dir)));
    }

    #[test]
    fn get_of_a_fifo_refuses_not_a_file() {
        let vfs = Mem::new();
        publish(&vfs, Path::new("/fifo"), b"");
        vfs.set_kind(Path::new("/fifo"), FileKind::Other)
            .expect("set_kind");

        let err = get(&vfs, Path::new("/fifo"), None).expect_err("must refuse");
        assert!(matches!(err, GetRefusal::NotAFile(FileKind::Other)));
    }

    #[test]
    fn get_over_max_bytes_refuses_too_large() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"0123456789");

        let err = get(&vfs, path, Some(5)).expect_err("must refuse");
        assert!(matches!(err, GetRefusal::TooLarge { size: 10, limit: 5 }));
    }

    #[test]
    fn get_unconfirmed_after_exhausting_retries_still_returns_bytes() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"before");
        vfs.set_churning(path, true);

        let sighting = get(&vfs, path, None).expect("get");
        assert!(!sighting.confirmed);
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

        let sighting = get(&vfs, path, None).expect("get");
        assert!(
            sighting.confirmed,
            "the bracket's retry must recover once the transient failure is consumed"
        );
    }

    #[test]
    fn get_settles_on_a_mutation_between_the_two_stats() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish(&vfs, path, b"hello");
        vfs.mutate_after_next_stat(path, b"after".to_vec());

        let sighting = get(&vfs, path, None).expect("get");
        assert!(sighting.confirmed);
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
    }

    #[test]
    fn get_reports_no_stat_when_the_bracket_stat_never_recovers() {
        let inner = Mem::new();
        publish(&inner, Path::new("/doc.md"), b"hello");
        let vfs = FailStatAfterFirstVfs {
            inner,
            stat_calls: std::sync::atomic::AtomicU32::new(0),
        };

        let sighting = get(&vfs, Path::new("/doc.md"), None).expect("get");
        assert!(!sighting.confirmed);
        assert!(sighting.stat.is_none());
    }
}
