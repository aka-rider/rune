use std::io;
use std::path::{Path, PathBuf};

use crate::sighting::{GetRefusal, Sighting, get};
use crate::{Etag, Stat, Vfs, etag_of, published_not_durable};

#[derive(Debug)]
pub enum PutCondition {
    IfMatch(Etag),
    IfAbsent,
    Force { expect: Option<Etag> },
}

#[derive(Debug)]
pub enum PutOutcome {
    Committed {
        etag: Etag,
        stat: Option<Stat>,
        durable: bool,
    },
    Conflict {
        current: Sighting,
    },
    Raced {
        etag: Etag,
        stat: Option<Stat>,
        durable: bool,
        displaced: Sighting,
    },
    Missing,
}

fn refusal_to_io(refusal: GetRefusal) -> io::Error {
    match refusal {
        GetRefusal::NotFound => io::Error::new(io::ErrorKind::NotFound, "not found"),
        GetRefusal::NotAFile(kind) => {
            io::Error::new(io::ErrorKind::InvalidInput, format!("not a file: {kind:?}"))
        }
        GetRefusal::TooLarge { size, limit } => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unexpected size refusal: {size} > {limit}"),
        ),
        GetRefusal::Io(e) => e,
    }
}

fn current_sighting<V: Vfs + ?Sized>(vfs: &V, path: &Path) -> io::Result<Option<Sighting>> {
    match get(vfs, path, None) {
        Ok(sighting) => Ok(Some(sighting)),
        Err(GetRefusal::NotFound) => Ok(None),
        Err(other) => Err(refusal_to_io(other)),
    }
}

type PutAndTemp = (PutOutcome, Option<PathBuf>);

fn finish_over_existing<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
    temp: &Path,
    publish: io::Result<()>,
    new_etag: &Etag,
    race_baseline: &Etag,
) -> io::Result<PutAndTemp> {
    let durable = match publish {
        Ok(()) => true,
        Err(e) if published_not_durable(&e) => false,
        Err(e) => {
            let _ = vfs.remove(temp);
            return Err(e);
        }
    };
    let displaced_bytes = vfs.read(temp)?;
    let displaced = Sighting {
        etag: etag_of(&displaced_bytes),
        stat: vfs.stat(temp).ok(),
        confirmed: true,
        bytes: displaced_bytes,
    };
    let stat = vfs.stat(path).ok();
    let outcome = if &displaced.etag != race_baseline {
        PutOutcome::Raced {
            etag: new_etag.clone(),
            stat,
            durable,
            displaced,
        }
    } else {
        PutOutcome::Committed {
            etag: new_etag.clone(),
            stat,
            durable,
        }
    };
    Ok((outcome, Some(temp.to_path_buf())))
}

fn put_if_match<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
    bytes: &[u8],
    expect: &Etag,
) -> io::Result<PutAndTemp> {
    let Some(mut sighting) = current_sighting(vfs, path)? else {
        return Ok((PutOutcome::Missing, None));
    };
    if !sighting.confirmed {
        let Some(retry) = current_sighting(vfs, path)? else {
            return Ok((PutOutcome::Missing, None));
        };
        sighting = retry;
    }
    if !sighting.confirmed || &sighting.etag != expect {
        return Ok((PutOutcome::Conflict { current: sighting }, None));
    }
    let dest = vfs.resolve(path)?;
    let temp = vfs.write_durable(&dest, bytes)?;
    let publish = vfs.exchange(&temp, &dest);
    let new_etag = etag_of(bytes);
    finish_over_existing(vfs, &dest, &temp, publish, &new_etag, expect)
}

fn put_if_absent<V: Vfs + ?Sized>(vfs: &V, path: &Path, bytes: &[u8]) -> io::Result<PutAndTemp> {
    let dest = vfs.resolve(path)?;
    let temp = vfs.write_durable(&dest, bytes)?;
    match vfs.rename_excl(&temp, &dest) {
        Ok(()) => Ok((
            PutOutcome::Committed {
                etag: etag_of(bytes),
                stat: vfs.stat(&dest).ok(),
                durable: true,
            },
            None,
        )),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let _ = vfs.remove(&temp);
            let current = current_sighting(vfs, &dest)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "winner vanished after AlreadyExists",
                )
            })?;
            Ok((PutOutcome::Conflict { current }, None))
        }
        Err(e) if published_not_durable(&e) => Ok((
            PutOutcome::Committed {
                etag: etag_of(bytes),
                stat: vfs.stat(&dest).ok(),
                durable: false,
            },
            None,
        )),
        Err(e) => Err(e),
    }
}

fn put_force<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
    bytes: &[u8],
    expect: Option<&Etag>,
) -> io::Result<PutAndTemp> {
    let dest = vfs.resolve(path)?;
    let dest_existed = vfs.stat(&dest).is_ok();
    let temp = vfs.write_durable(&dest, bytes)?;
    if !dest_existed {
        return match vfs.rename_excl(&temp, &dest) {
            Ok(()) => Ok((
                PutOutcome::Committed {
                    etag: etag_of(bytes),
                    stat: vfs.stat(&dest).ok(),
                    durable: true,
                },
                None,
            )),
            Err(e) if published_not_durable(&e) => Ok((
                PutOutcome::Committed {
                    etag: etag_of(bytes),
                    stat: vfs.stat(&dest).ok(),
                    durable: false,
                },
                None,
            )),
            Err(e) => {
                let _ = vfs.remove(&temp);
                Err(e)
            }
        };
    }
    let publish = vfs.exchange(&temp, &dest);
    let new_etag = etag_of(bytes);
    let race_baseline = expect.cloned().unwrap_or_else(|| new_etag.clone());
    finish_over_existing(vfs, &dest, &temp, publish, &new_etag, &race_baseline)
}

pub(crate) fn put_and_temp<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
    bytes: &[u8],
    cond: PutCondition,
) -> io::Result<PutAndTemp> {
    match cond {
        PutCondition::IfMatch(expect) => put_if_match(vfs, path, bytes, &expect),
        PutCondition::IfAbsent => put_if_absent(vfs, path, bytes),
        PutCondition::Force { expect } => put_force(vfs, path, bytes, expect.as_ref()),
    }
}

pub fn put<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
    bytes: &[u8],
    cond: PutCondition,
) -> io::Result<PutOutcome> {
    put_and_temp(vfs, path, bytes, cond).map(|(outcome, _)| outcome)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{Identity, Mem, OpKind};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn publish_direct(vfs: &Mem, path: &Path, bytes: &[u8]) {
        let temp = vfs.write_durable(path, bytes).expect("write_durable");
        vfs.rename_excl(&temp, path).expect("publish");
    }

    #[test]
    fn if_match_conflict_on_hash_mismatch() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish_direct(&vfs, path, b"original");

        let outcome = put(&vfs, path, b"new", PutCondition::IfMatch(etag_of(b"wrong"))).unwrap();
        assert!(matches!(outcome, PutOutcome::Conflict { .. }));
    }

    #[test]
    fn if_match_missing_destination_returns_missing() {
        let vfs = Mem::new();
        let outcome = put(
            &vfs,
            Path::new("/gone.md"),
            b"new",
            PutCondition::IfMatch(etag_of(b"whatever")),
        )
        .unwrap();
        assert!(matches!(outcome, PutOutcome::Missing));
    }

    #[test]
    fn if_match_committed_replaces_matching_content() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish_direct(&vfs, path, b"original");

        let outcome = put(
            &vfs,
            path,
            b"updated",
            PutCondition::IfMatch(etag_of(b"original")),
        )
        .unwrap();
        assert!(matches!(
            outcome,
            PutOutcome::Committed { durable: true, .. }
        ));
        assert_eq!(vfs.read(path).unwrap(), b"updated");
    }

    struct FlappingIdentityVfs {
        inner: Mem,
        calls: AtomicU64,
    }

    impl Vfs for FlappingIdentityVfs {
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
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            stat.identity = Identity {
                inode: Some(n),
                device: Some(1),
            };
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
    }

    #[test]
    fn if_match_refuses_an_unconfirmed_read_even_when_the_hash_matches() {
        let inner = Mem::new();
        publish_direct(&inner, Path::new("/doc.md"), b"stable content");
        let vfs = FlappingIdentityVfs {
            inner,
            calls: AtomicU64::new(0),
        };

        let outcome = put(
            &vfs,
            Path::new("/doc.md"),
            b"new",
            PutCondition::IfMatch(etag_of(b"stable content")),
        )
        .unwrap();
        assert!(matches!(outcome, PutOutcome::Conflict { .. }));
    }

    #[test]
    fn force_committed_reports_durable_false_on_a_post_publish_durability_failure() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish_direct(&vfs, path, b"original");
        vfs.fail_after(OpKind::Exchange, io::ErrorKind::Other);

        let outcome = put(
            &vfs,
            path,
            b"original",
            PutCondition::Force { expect: None },
        )
        .unwrap();
        assert!(matches!(
            outcome,
            PutOutcome::Committed { durable: false, .. }
        ));
        assert_eq!(vfs.read(path).unwrap(), b"original");
    }

    #[test]
    fn if_absent_loser_gets_conflict_and_the_temp_is_removed() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish_direct(&vfs, path, b"winner");

        let before = vfs.debug_paths().len();
        let outcome = put(&vfs, path, b"loser", PutCondition::IfAbsent).unwrap();
        let PutOutcome::Conflict { current } = outcome else {
            unreachable!("expected Conflict, got {outcome:?}");
        };
        assert_eq!(current.bytes, b"winner");
        assert_eq!(vfs.debug_paths().len(), before);
    }

    #[test]
    fn if_absent_non_collision_failure_keeps_the_temp() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        vfs.fail_next(OpKind::RenameExcl, io::ErrorKind::PermissionDenied);

        let before = vfs.debug_paths().len();
        let result = put(&vfs, path, b"bytes", PutCondition::IfAbsent);
        assert!(result.is_err());
        assert_eq!(vfs.debug_paths().len(), before + 1);
    }

    #[test]
    fn force_with_matching_expect_commits() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish_direct(&vfs, path, b"original");

        let outcome = put(
            &vfs,
            path,
            b"updated",
            PutCondition::Force {
                expect: Some(etag_of(b"original")),
            },
        )
        .unwrap();
        assert!(matches!(
            outcome,
            PutOutcome::Committed { durable: true, .. }
        ));
        assert_eq!(vfs.read(path).unwrap(), b"updated");
    }

    #[test]
    fn force_over_foreign_bytes_races_with_displaced_captured() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish_direct(&vfs, path, b"original");
        // A foreign writer replaces the content out from under the caller's
        // recorded baseline before the Force publish runs.
        let foreign_temp = vfs.write_durable(path, b"foreign").unwrap();
        vfs.exchange(&foreign_temp, path).unwrap();
        let _ = vfs.remove(&foreign_temp);

        let outcome = put(
            &vfs,
            path,
            b"mine",
            PutCondition::Force {
                expect: Some(etag_of(b"original")),
            },
        )
        .unwrap();
        let PutOutcome::Raced { displaced, .. } = outcome else {
            unreachable!("expected Raced, got {outcome:?}");
        };
        assert_eq!(displaced.bytes, b"foreign");
        assert_eq!(vfs.read(path).unwrap(), b"mine");
    }

    #[test]
    fn force_fresh_create_commits() {
        let vfs = Mem::new();
        let outcome = put(
            &vfs,
            Path::new("/new.md"),
            b"content",
            PutCondition::Force { expect: None },
        )
        .unwrap();
        assert!(matches!(
            outcome,
            PutOutcome::Committed { durable: true, .. }
        ));
    }

    #[test]
    fn two_sequential_puts_to_one_path_never_collide_on_temp_names() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        publish_direct(&vfs, path, b"one");

        put(&vfs, path, b"two", PutCondition::Force { expect: None }).unwrap();
        put(&vfs, path, b"three", PutCondition::Force { expect: None }).unwrap();

        assert_eq!(vfs.read(path).unwrap(), b"three");
    }

    #[test]
    fn if_match_over_a_directory_refuses_with_invalid_input() {
        let vfs = Mem::new();
        publish_direct(&vfs, Path::new("/a/b.md"), b"content");

        let result = put(
            &vfs,
            Path::new("/a"),
            b"anything",
            PutCondition::IfMatch(etag_of(b"anything")),
        );
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }
}
