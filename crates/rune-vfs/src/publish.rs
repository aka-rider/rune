use std::io;
use std::path::{Path, PathBuf};

use crate::sighting::{GetRefusal, Sighted, Sighting, bracketed_stat, get_resolved};
use crate::{Etag, FileKind, Vfs, etag_of, published_not_durable};

pub(crate) use crate::put_result::{ForceOutcome, IfAbsentOutcome, Published};

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
        sighted: Sighted,
        durable: bool,
        stray_temp: Option<PathBuf>,
    },
    Conflict {
        current: Sighting,
        stray_temp: Option<PathBuf>,
    },
    Raced {
        etag: Etag,
        sighted: Sighted,
        durable: bool,
        stray_temp: Option<PathBuf>,
        displaced: Sighting,
    },
    Missing,
}

/// Resolves `path` once, treating a not-found-shaped resolve failure (a
/// missing ancestor directory) as a plain absence rather than an error — the
/// same case `current_sighting` used to fold into `PutOutcome::Missing` via
/// `get`'s own resolve. Callers publish over the SAME `PathBuf` this returns
/// instead of resolving `path` again, closing the symlink-swap TOCTOU window
/// a second, independent `Vfs::resolve` call would reopen.
fn resolve_or_missing<V: Vfs + ?Sized>(vfs: &V, path: &Path) -> io::Result<Option<PathBuf>> {
    match vfs.resolve(path) {
        Ok(resolved) => Ok(Some(resolved)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// `get_resolved` against a path the caller already resolved — see
/// `resolve_or_missing`. `resolved` MUST be that single resolution, never a
/// fresh, independently-resolved path.
fn current_sighting<V: Vfs + ?Sized>(vfs: &V, resolved: &Path) -> io::Result<Option<Sighting>> {
    match get_resolved(vfs, resolved, None) {
        Ok(sighting) => Ok(Some(sighting)),
        Err(GetRefusal::NotFound) => Ok(None),
        Err(other) => Err(other.into()),
    }
}

fn remove_temp_noting_failure<V: Vfs + ?Sized>(vfs: &V, temp: &Path, e: io::Error) -> io::Error {
    match vfs.remove(temp) {
        Ok(()) => e,
        Err(cleanup_err) => crate::wrap_io(
            e,
            format!(
                "stray temp {} could not be cleaned up either: {cleanup_err}",
                temp.display()
            ),
        ),
    }
}

fn finish_over_existing<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
    temp: &Path,
    publish: io::Result<()>,
    new_etag: &Etag,
    race_baseline: &Etag,
) -> io::Result<ForceOutcome> {
    let durable = match publish {
        Ok(()) => true,
        Err(e) if published_not_durable(&e) => false,
        Err(e) => return Err(remove_temp_noting_failure(vfs, temp, e)),
    };
    let sighted = bracketed_stat(vfs, path);
    let mut published = Published {
        etag: new_etag.clone(),
        sighted,
        durable,
        stray_temp: None,
    };
    // The publish already took effect: a failure reading the displaced
    // bytes back off the temp is NOT a failed save. The temp is kept — it
    // may hold the sole copy of the displaced content — and the raced-ness
    // stays unclassified, so this reports a plain commit that names the
    // kept temp via `stray_temp`.
    let Ok(displaced_bytes) = vfs.read(temp) else {
        published.stray_temp = Some(temp.to_path_buf());
        return Ok(ForceOutcome::Committed(published));
    };
    let displaced_sighted = vfs
        .stat(temp)
        .map_or(Sighted::Unconfirmed(None), Sighted::Confirmed);
    let displaced = Sighting {
        etag: etag_of(&displaced_bytes),
        sighted: displaced_sighted,
        bytes: displaced_bytes,
    };
    if durable && vfs.remove(temp).is_err() {
        published.stray_temp = Some(temp.to_path_buf());
    }
    let outcome = if &displaced.etag != race_baseline {
        ForceOutcome::Raced {
            published,
            displaced,
        }
    } else {
        ForceOutcome::Committed(published)
    };
    Ok(outcome)
}

pub(crate) fn put_if_match<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
    bytes: &[u8],
    expect: &Etag,
) -> io::Result<PutOutcome> {
    let Some(dest) = resolve_or_missing(vfs, path)? else {
        return Ok(PutOutcome::Missing);
    };
    let Some(mut sighting) = current_sighting(vfs, &dest)? else {
        return Ok(PutOutcome::Missing);
    };
    if !sighting.sighted.is_confirmed() || &sighting.etag != expect {
        let Some(retry) = current_sighting(vfs, &dest)? else {
            return Ok(PutOutcome::Missing);
        };
        sighting = retry;
    }
    if !sighting.sighted.is_confirmed() || &sighting.etag != expect {
        return Ok(PutOutcome::Conflict {
            current: sighting,
            stray_temp: None,
        });
    }
    let temp = vfs.write_durable(&dest, bytes)?;
    let publish = vfs.exchange(&temp, &dest);
    let new_etag = etag_of(bytes);
    let landed = finish_over_existing(vfs, &dest, &temp, publish, &new_etag, expect)?;
    Ok(landed.into())
}

fn finish_fresh_create<V: Vfs + ?Sized>(
    vfs: &V,
    dest: &Path,
    temp: &Path,
    bytes: &[u8],
    publish: io::Result<()>,
) -> io::Result<Published> {
    let durable = match publish {
        Ok(()) => true,
        Err(e) if published_not_durable(&e) => false,
        Err(e) => return Err(remove_temp_noting_failure(vfs, temp, e)),
    };
    Ok(Published {
        etag: etag_of(bytes),
        sighted: bracketed_stat(vfs, dest),
        durable,
        stray_temp: None,
    })
}

pub(crate) fn put_if_absent<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
    bytes: &[u8],
) -> io::Result<IfAbsentOutcome> {
    let dest = vfs.resolve(path)?;
    let temp = vfs.write_durable(&dest, bytes)?;
    let publish = match vfs.rename_excl(&temp, &dest) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let stray_temp = vfs.remove(&temp).err().map(|_| temp.clone());
            let current = current_sighting(vfs, &dest)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "winner vanished after AlreadyExists",
                )
            })?;
            return Ok(IfAbsentOutcome::Conflict {
                current,
                stray_temp,
            });
        }
        other => other,
    };
    let published = finish_fresh_create(vfs, &dest, &temp, bytes, publish)?;
    Ok(IfAbsentOutcome::Committed(published))
}

pub(crate) fn put_force<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
    bytes: &[u8],
    expect: Option<&Etag>,
) -> io::Result<ForceOutcome> {
    let dest = vfs.resolve(path)?;
    let dest_stat = vfs.stat(&dest).ok();
    let temp = vfs.write_durable(&dest, bytes)?;
    let Some(dest_stat) = dest_stat else {
        let publish = vfs.rename_excl(&temp, &dest);
        let published = finish_fresh_create(vfs, &dest, &temp, bytes, publish)?;
        return Ok(ForceOutcome::Committed(published));
    };
    if dest_stat.kind != FileKind::File {
        let refusal: io::Error = GetRefusal::NotAFile(dest_stat.kind).into();
        return Err(remove_temp_noting_failure(vfs, &temp, refusal));
    }
    let publish = vfs.exchange(&temp, &dest);
    let new_etag = etag_of(bytes);
    let race_baseline = expect.cloned().unwrap_or_else(|| new_etag.clone());
    finish_over_existing(vfs, &dest, &temp, publish, &new_etag, &race_baseline)
}

pub fn put<V: Vfs + ?Sized>(
    vfs: &V,
    path: &Path,
    bytes: &[u8],
    cond: PutCondition,
) -> io::Result<PutOutcome> {
    match cond {
        PutCondition::IfMatch(expect) => put_if_match(vfs, path, bytes, &expect),
        PutCondition::IfAbsent => put_if_absent(vfs, path, bytes).map(Into::into),
        PutCondition::Force { expect } => {
            put_force(vfs, path, bytes, expect.as_ref()).map(Into::into)
        }
    }
}

#[cfg(test)]
#[path = "publish_tests.rs"]
mod tests;
