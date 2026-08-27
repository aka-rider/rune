use std::io;
use std::path::Path;

use rune_core::assert_invariant;

use crate::{Etag, FileKind, Stat, Vfs, etag_of};

pub(crate) const BRACKET_MAX_ATTEMPTS: u32 = 3;

pub const MAX_DOCUMENT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sighted {
    Confirmed(Stat),
    Unconfirmed(Option<Stat>),
}

impl Sighted {
    pub fn stat(&self) -> Option<Stat> {
        match self {
            Sighted::Confirmed(stat) => Some(*stat),
            Sighted::Unconfirmed(stat) => *stat,
        }
    }

    pub fn is_confirmed(&self) -> bool {
        matches!(self, Sighted::Confirmed(_))
    }
}

fn sighted(stable: bool, stat: Option<Stat>) -> Sighted {
    match (stable, stat) {
        (true, Some(stat)) => Sighted::Confirmed(stat),
        (true, None) => {
            assert_invariant!(false, || {
                "a confirmed sighting must carry a stat".to_string()
            });
            Sighted::Unconfirmed(None)
        }
        (false, stat) => Sighted::Unconfirmed(stat),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sighting {
    pub bytes: Vec<u8>,
    pub etag: Etag,
    pub sighted: Sighted,
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

impl From<GetRefusal> for io::Error {
    fn from(refusal: GetRefusal) -> io::Error {
        if let GetRefusal::Io(e) = refusal {
            return e;
        }
        let kind = match &refusal {
            GetRefusal::NotFound => io::ErrorKind::NotFound,
            GetRefusal::NotAFile(FileKind::Dir) => io::ErrorKind::IsADirectory,
            GetRefusal::NotAFile(_) => io::ErrorKind::InvalidInput,
            GetRefusal::TooLarge { .. } => io::ErrorKind::FileTooLarge,
            GetRefusal::Io(_) => unreachable!("handled above"),
        };
        io::Error::new(kind, refusal)
    }
}

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

fn one_get_bracket<V: Vfs + ?Sized>(vfs: &V, path: &Path) -> io::Result<(Vec<u8>, Sighted)> {
    let before = stat_with_identity(vfs, path);
    let bytes = vfs.read(path)?;
    let after = stat_with_identity(vfs, path);
    let confirmed = matches!((&before, &after), (Some(b), Some(a)) if stat_matches(b, a));
    let stat = after.or(before);
    Ok((bytes, sighted(confirmed, stat)))
}

fn bracketed_get<V: Vfs + ?Sized>(vfs: &V, path: &Path) -> io::Result<(Vec<u8>, Sighted)> {
    let mut result = one_get_bracket(vfs, path)?;
    let mut attempts = 1;
    while !result.1.is_confirmed() && attempts < BRACKET_MAX_ATTEMPTS {
        result = one_get_bracket(vfs, path)?;
        attempts += 1;
    }
    Ok(result)
}

pub(crate) fn bracketed_stat<V: Vfs + ?Sized>(vfs: &V, path: &Path) -> Sighted {
    let mut last = None;
    for _ in 0..BRACKET_MAX_ATTEMPTS {
        let first = stat_with_identity(vfs, path);
        let second = stat_with_identity(vfs, path);
        if let (Some(a), Some(b)) = (&first, &second)
            && stat_matches(a, b)
        {
            return sighted(true, second);
        }
        last = second.or(first);
    }
    sighted(false, last)
}

pub fn get<V: Vfs + ?Sized>(vfs: &V, path: &Path, max_bytes: u64) -> Result<Sighting, GetRefusal> {
    let resolved = vfs.resolve(path).map_err(as_not_found)?;
    get_resolved(vfs, &resolved, max_bytes)
}

/// [`get`]'s body, minus the resolve — for a caller that already resolved
/// `resolved` itself and must not resolve it a second time (a second,
/// independent `Vfs::resolve` call is a symlink-swap TOCTOU window: the
/// target the caller decided to open could stop being the target this
/// function reads). `resolved` MUST already be the caller's own
/// `Vfs::resolve` output, not an arbitrary path.
pub fn get_resolved<V: Vfs + ?Sized>(
    vfs: &V,
    resolved: &Path,
    max_bytes: u64,
) -> Result<Sighting, GetRefusal> {
    let stat = vfs.stat(resolved).map_err(as_not_found)?;
    if stat.kind != FileKind::File {
        return Err(GetRefusal::NotAFile(stat.kind));
    }
    if stat.size > max_bytes {
        return Err(GetRefusal::TooLarge {
            size: stat.size,
            limit: max_bytes,
        });
    }
    let (bytes, sighted) = bracketed_get(vfs, resolved).map_err(GetRefusal::Io)?;
    let etag = etag_of(&bytes);
    Ok(Sighting {
        bytes,
        etag,
        sighted,
    })
}

#[cfg(test)]
#[path = "sighting_tests.rs"]
mod tests;
