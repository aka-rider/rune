use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::mem::MemFile;
use crate::{FileKind, Link, MAX_SYMLINK_HOPS};

/// See `Mem::resolve`. Anchors `path` at a synthetic root and collapses
/// `.`/`..` components against what came before, entirely lexically (no
/// filesystem access — `Mem` has none). A `..` past the root has nowhere to
/// go and is dropped, the same shape `Path::components()` already gives an
/// absolute path.
pub(crate) fn lexically_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.last(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            Component::RootDir => {
                out.clear();
                out.push(Component::RootDir);
            }
            Component::Normal(_) | Component::Prefix(_) => out.push(component),
        }
    }
    if !matches!(out.first(), Some(Component::RootDir)) {
        out.insert(0, Component::RootDir);
    }
    out.into_iter().collect()
}

/// `Mem` has no directory nodes (`MemState.files` is a flat
/// `HashMap<PathBuf, MemFile>`), so a directory exists at `path` iff some
/// stored key sits strictly below it — i.e. `key` starts with `path` plus at
/// least one more component.
pub(crate) fn sits_strictly_below(key: &Path, path: &Path) -> bool {
    key.strip_prefix(path)
        .is_ok_and(|rest| rest.components().next().is_some())
}

/// Where a symlink stored at `link` points: an absolute target as written, a
/// relative one against the link's own parent — matching
/// `std::os::unix::fs::symlink`.
pub(crate) fn link_destination(link: &Path, target: &Path) -> PathBuf {
    if target.is_absolute() {
        return lexically_normalize(target);
    }
    let parent = link.parent().unwrap_or(Path::new("/"));
    lexically_normalize(&parent.join(target))
}

/// Walks `path` component by component, replacing every component that names a
/// symlink with its destination, until no link remains or the hop budget runs
/// out. A path that touched no link is returned untouched, so a `Mem` keyed by
/// exact spelling keeps answering the way it always did.
pub(crate) fn follow_links(files: &HashMap<PathBuf, MemFile>, path: &Path) -> io::Result<PathBuf> {
    let mut walked = PathBuf::new();
    let mut hops = 0usize;
    let mut followed = false;
    for component in path.components() {
        walked.push(component);
        while let Some(target) = files.get(&walked).and_then(|f| f.link_target.clone()) {
            hops += 1;
            if hops > MAX_SYMLINK_HOPS {
                // `io::ErrorKind::FilesystemLoop` cannot be named on stable
                // rust; `ELOOP` is the same kind, spelled through the OS.
                return Err(io::Error::from_raw_os_error(libc::ELOOP));
            }
            walked = link_destination(&walked, &target);
            followed = true;
        }
    }
    Ok(if followed { walked } else { path.to_path_buf() })
}

/// The kind of what `path` names, or `None` when nothing is there. A synthetic
/// directory (some stored key sits below `path`) outranks a stored file of the
/// same name, so an inconsistent-but-representable `Mem` state answers the same
/// way whichever key the map iterates first.
pub(crate) fn kind_at(files: &HashMap<PathBuf, MemFile>, path: &Path) -> Option<FileKind> {
    if files.keys().any(|key| sits_strictly_below(key, path)) {
        return Some(FileKind::Dir);
    }
    files.get(path).map(|f| f.kind)
}

pub(crate) fn classify(files: &HashMap<PathBuf, MemFile>, path: &Path) -> (FileKind, Link) {
    let is_link = files.get(path).is_some_and(|f| f.link_target.is_some());
    if !is_link {
        return (kind_at(files, path).unwrap_or(FileKind::File), Link::No);
    }
    match follow_links(files, path)
        .ok()
        .and_then(|t| kind_at(files, &t))
    {
        Some(kind) => (kind, Link::To),
        None => (FileKind::Other, Link::Broken),
    }
}

pub(crate) fn not_found(path: &Path, op: &str) -> io::Error {
    crate::wrap_io(
        io::Error::new(io::ErrorKind::NotFound, "not found in mem vfs"),
        format!("{op} {}", path.display()),
    )
}
