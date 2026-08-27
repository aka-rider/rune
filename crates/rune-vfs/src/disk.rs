//! Disk-backed `Vfs` for Darwin and Linux, using a flagged atomic rename
//! (`renamex_np` on Darwin, `renameat2` on Linux) for atomic publish.

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::{
    DirEntry, FileKind, Identity, Link, MAX_SYMLINK_HOPS, Stat, Vfs, sort_dir_entries, temp_name,
};

/// Disk-backed `Vfs`. Uses a flagged atomic rename syscall for crash-safe
/// publish; stateless (no synchronization needed).
#[derive(Clone, Copy, Default)]
pub struct Disk;

impl Disk {
    fn fsync_dir(dir: &Path) -> io::Result<()> {
        let dir_file = File::open(dir)?;
        dir_file.sync_all()
    }

    /// Parent directory to fsync for a publish onto `path`: `path`'s own
    /// parent, or `.` for a bare relative filename (`parent()` is `Some("")`
    /// in that case, and fsyncing an empty path would fail).
    fn parent_to_fsync(path: &Path) -> PathBuf {
        match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        }
    }

    /// The only `unsafe` block in this crate. Wraps the Darwin
    /// `renamex_np` syscall to atomically exchange or create files with
    /// proper crash-safety semantics.
    #[cfg(target_os = "macos")]
    fn flagged_rename(src: &Path, dst: &Path, flags: libc::c_uint) -> io::Result<()> {
        let src_c = CString::new(src.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let dst_c = CString::new(dst.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let ret = unsafe { libc::renamex_np(src_c.as_ptr(), dst_c.as_ptr(), flags) };
        if ret == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// The only `unsafe` block in this crate. Wraps the Linux `renameat2`
    /// syscall to atomically exchange or create files with proper
    /// crash-safety semantics.
    #[cfg(target_os = "linux")]
    fn flagged_rename(src: &Path, dst: &Path, flags: libc::c_uint) -> io::Result<()> {
        let src_c = CString::new(src.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let dst_c = CString::new(dst.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let ret = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                src_c.as_ptr(),
                libc::AT_FDCWD,
                dst_c.as_ptr(),
                flags,
            )
        };
        if ret == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Shared shape behind [`Vfs::exchange`]/[`Vfs::rename_excl`]: both are
    /// a flagged atomic rename under a different flag, followed by a
    /// parent-directory fsync — they differ only in the flag and the label
    /// their error messages report. `label` reads as `"{label} {src} ->
    /// {dst}: ..."`.
    fn publish(src: &Path, dst: &Path, flags: libc::c_uint, label: &str) -> io::Result<()> {
        Self::flagged_rename(src, dst, flags).map_err(|e| {
            crate::wrap_io(e, format!("{label} {} -> {}", src.display(), dst.display()))
        })?;
        Self::fsync_dir(&Self::parent_to_fsync(dst)).map_err(|e| {
            // WP1.S1: the rename/swap above already succeeded — only the
            // durability confirmation (parent fsync) failed. `dst` already
            // holds the new content, and (for an exchange) `src` already
            // holds whatever `dst` displaced: mark the error so a caller
            // composing on top of `publish` (e.g. `save_atomic`) knows a
            // temp file named by `src`/`dst` must not be discarded.
            crate::wrap_io_published(
                e,
                format!(
                    "{label} {} -> {}: fsync parent",
                    src.display(),
                    dst.display()
                ),
            )
        })
    }

    /// See [`Vfs::resolve`]. `hops` counts symlink substitutions made so
    /// far, mirroring `Mem::resolve`'s own `MAX_SYMLINK_HOPS` cycle guard —
    /// a self-referential or mutually-dangling symlink chain must error
    /// `ELOOP` rather than recurse forever.
    fn resolve_leaf(path: &Path, hops: usize) -> io::Result<PathBuf> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let hops = hops + 1;
                if hops > MAX_SYMLINK_HOPS {
                    return Err(io::Error::from_raw_os_error(libc::ELOOP));
                }
                let raw_target = fs::read_link(path).map_err(|e| {
                    crate::wrap_io(e, format!("resolve {}: read_link", path.display()))
                })?;
                let target = if raw_target.is_absolute() {
                    raw_target
                } else {
                    path.parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(raw_target)
                };
                Self::resolve_leaf(&target, hops)
            }
            Ok(_) => fs::canonicalize(path)
                .map_err(|e| crate::wrap_io(e, format!("resolve {}", path.display()))),
            Err(_) => match path.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => {
                    let canonical_parent = fs::canonicalize(parent).map_err(|e| {
                        crate::wrap_io(e, format!("resolve {}: resolve parent", path.display()))
                    })?;
                    Ok(canonical_parent.join(path.file_name().unwrap_or_default()))
                }
                _ => {
                    let canonical_cwd = std::env::current_dir().map_err(|e| {
                        crate::wrap_io(
                            e,
                            format!("resolve {}: get current directory", path.display()),
                        )
                    })?;
                    Ok(canonical_cwd.join(path))
                }
            },
        }
    }
}

impl Vfs for Disk {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path).map_err(|e| crate::wrap_io(e, format!("read {}", path.display())))
    }

    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        let temp = temp_name(path);

        // `create_new(true)` alone guarantees a brand-new file (it errors
        // `AlreadyExists` rather than opening one that's already there), so
        // there is never an existing file for `truncate(true)` to act on —
        // that option is deliberately absent. `mode(0o600)` closes the
        // window where the temp — full document plaintext — would
        // otherwise sit at the umask-derived default (typically
        // world-readable) until the permission copy below runs.
        let mut temp_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC)
            .open(&temp)?;

        use std::io::Write;
        // Best-effort cleanup of a never-published temp on a write failure:
        // the destination file is untouched either way, so a failed remove
        // here just leaks a scratch file (disk hygiene), never user bytes.
        temp_file.write_all(bytes).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })?;

        temp_file.sync_all().inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })?;

        drop(temp_file);

        // Preserve the destination's permissions on the temp before it's
        // ever published, so a SWAP publish doesn't change the file's
        // mode: after the publish, the destination IS the temp's inode, so
        // a silently-swallowed failure here would permanently downgrade
        // (or upgrade) the published file's mode with nothing to show for
        // it. A brand-new document (no existing destination) has nothing
        // to preserve and keeps the `mode(0o600)` set above.
        if let Ok(metadata) = fs::metadata(path) {
            fs::set_permissions(&temp, metadata.permissions()).map_err(|e| {
                let _ = fs::remove_file(&temp);
                crate::wrap_io(
                    e,
                    format!(
                        "write_durable {}: could not preserve the destination's permissions on the temp",
                        path.display()
                    ),
                )
            })?;
        }

        Ok(temp)
    }

    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        Self::publish(a, b, swap_flag(), "exchange")
    }

    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()> {
        Self::publish(old, new, excl_flag(), "renameexcl")
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path).map_err(|e| crate::wrap_io(e, format!("remove {}", path.display())))
    }

    #[cfg(target_os = "macos")]
    fn trash(&self, path: &Path) -> io::Result<()> {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};

        let mut ctx = trash::TrashContext::default();
        // Finder's default method shells out to `osascript`, which triggers
        // a macOS automation-permission dialog against the host terminal;
        // NsFileManager trashes silently with no subprocess. The item is
        // still fully recoverable — only Finder's "Put Back" entry is lost.
        ctx.set_delete_method(DeleteMethod::NsFileManager);
        ctx.delete(path)
            .map_err(|e| io::Error::other(describe_trash_error(&e)))
    }

    #[cfg(not(target_os = "macos"))]
    fn trash(&self, path: &Path) -> io::Result<()> {
        trash::delete(path).map_err(|e| io::Error::other(describe_trash_error(&e)))
    }

    fn stat(&self, path: &Path) -> io::Result<Stat> {
        let meta = fs::metadata(path)?;
        Ok(Stat {
            size: meta.len(),
            mtime: meta.modified()?,
            identity: Identity {
                inode: Some(meta.ino()),
                device: Some(meta.dev()),
            },
            nlink: Some(meta.nlink()),
            kind: kind_of(&meta),
        })
    }

    fn read_link(&self, path: &Path) -> io::Result<PathBuf> {
        fs::read_link(path).map_err(|e| crate::wrap_io(e, format!("read_link {}", path.display())))
    }

    /// Canonicalize `path`, following a symlink leaf to its target even when
    /// that target doesn't exist (a dangling link) — the POSIX-editor
    /// convention: writing through a dangling symlink creates the target,
    /// never the link itself. When the final, non-symlink leaf doesn't
    /// exist yet (first save of a brand-new file, or the dangling target
    /// just followed to), only its parent directory is resolved and the
    /// unresolved leaf name is re-joined, so a symlinked parent directory
    /// still canonicalizes correctly.
    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        Self::resolve_leaf(path, 0)
    }

    fn mkdir_all(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
            .map_err(|e| crate::wrap_io(e, format!("mkdir_all {}", path.display())))
    }

    /// `path` itself failing to open propagates (the caller needs to know
    /// the listing didn't happen at all). A single entry's `file_type()`
    /// failing — e.g. it vanished between the readdir syscall and the stat
    /// (TOCTOU), or a permissions edge case — only skips that one entry:
    /// the rest of the listing is still real information the caller can
    /// use, so one flaky entry shouldn't fail the whole call.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let mut entries = Vec::new();
        let dir = fs::read_dir(path)
            .map_err(|e| crate::wrap_io(e, format!("read_dir {}", path.display())))?;
        for entry in dir {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let (kind, link) = match entry.file_type() {
                Ok(ft) if ft.is_dir() => (FileKind::Dir, Link::No),
                Ok(ft) if ft.is_file() => (FileKind::File, Link::No),
                Ok(ft) if ft.is_symlink() => match fs::metadata(entry.path()) {
                    Ok(target) => (kind_of(&target), Link::To),
                    Err(_) => (FileKind::Other, Link::Broken),
                },
                Ok(_) => (FileKind::Other, Link::No),
                Err(_) => continue,
            };
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                path: entry.path(),
                kind,
                link,
            });
        }
        sort_dir_entries(&mut entries);
        Ok(entries)
    }
}

/// A FIFO, socket or device node is neither file nor directory: it must never
/// be offered as a link target, because reading one never returns.
fn kind_of(meta: &fs::Metadata) -> FileKind {
    if meta.is_dir() {
        FileKind::Dir
    } else if meta.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    }
}

#[cfg(target_os = "macos")]
fn swap_flag() -> libc::c_uint {
    libc::RENAME_SWAP
}

#[cfg(target_os = "macos")]
fn excl_flag() -> libc::c_uint {
    libc::RENAME_EXCL
}

#[cfg(target_os = "linux")]
fn swap_flag() -> libc::c_uint {
    libc::RENAME_EXCHANGE
}

#[cfg(target_os = "linux")]
fn excl_flag() -> libc::c_uint {
    libc::RENAME_NOREPLACE
}

/// `trash::Error`'s `Display` is a raw `Debug` dump — map the variants a
/// user-facing message can actually explain, falling back to a short
/// unqualified label for the rest rather than surfacing the dump.
fn describe_trash_error(error: &trash::Error) -> String {
    match error {
        trash::Error::Os { description, .. } | trash::Error::Unknown { description } => {
            description.clone()
        }
        trash::Error::CouldNotAccess { target } => format!("could not access {target}"),
        trash::Error::TargetedRoot => "refused to trash a root folder".to_string(),
        trash::Error::CanonicalizePath { original } => {
            format!("could not resolve path {}", original.display())
        }
        _ => "trash operation failed".to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod describe_trash_error_tests {
    use super::describe_trash_error;

    #[test]
    fn os_and_unknown_pass_their_description_through_verbatim() {
        assert_eq!(
            describe_trash_error(&trash::Error::Os {
                code: 13,
                description: "permission denied".to_string(),
            }),
            "permission denied"
        );
        assert_eq!(
            describe_trash_error(&trash::Error::Unknown {
                description: "something odd".to_string(),
            }),
            "something odd"
        );
    }

    #[test]
    fn could_not_access_names_the_target() {
        assert_eq!(
            describe_trash_error(&trash::Error::CouldNotAccess {
                target: "/some/path".to_string(),
            }),
            "could not access /some/path"
        );
    }

    #[test]
    fn targeted_root_has_a_fixed_message() {
        assert_eq!(
            describe_trash_error(&trash::Error::TargetedRoot),
            "refused to trash a root folder"
        );
    }

    #[test]
    fn canonicalize_path_names_the_original_path() {
        assert_eq!(
            describe_trash_error(&trash::Error::CanonicalizePath {
                original: std::path::PathBuf::from("/weird/path"),
            }),
            "could not resolve path /weird/path"
        );
    }
}
