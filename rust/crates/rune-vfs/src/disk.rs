//! Darwin-specific disk-backed `Vfs` using `renamex_np` for atomic publish.
//!
//! Port of Go's `pkg/vfs/exchange_darwin.go` and `pkg/vfs/disk.go`.

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::{Identity, Stat, Vfs, temp_name};

/// Disk-backed `Vfs` on Darwin. Uses `renamex_np` for crash-safe atomic
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

    /// The only `unsafe` block in `rune-vfs` (`rune-db::session` has three
    /// of its own, for `sysctl`/`kill`-based liveness checks). Wraps the
    /// Darwin `renamex_np` syscall to atomically exchange or create files
    /// with proper crash-safety semantics.
    fn renamex_np(src: &Path, dst: &Path, flags: libc::c_uint) -> io::Result<()> {
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

    /// Shared shape behind [`Vfs::exchange`]/[`Vfs::rename_excl`]: both are
    /// `renamex_np` under a different flag, followed by a parent-directory
    /// fsync (§1.4.1) — they differ only in the flag and the label their
    /// error messages report. `label` reads as `"{label} {src} -> {dst}:
    /// ..."`.
    fn publish(src: &Path, dst: &Path, flags: libc::c_uint, label: &str) -> io::Result<()> {
        Self::renamex_np(src, dst, flags).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("{label} {} -> {}: {}", src.display(), dst.display(), e),
            )
        })?;
        Self::fsync_dir(&Self::parent_to_fsync(dst)).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "{label} {} -> {}: fsync parent: {}",
                    src.display(),
                    dst.display(),
                    e
                ),
            )
        })
    }
}

impl Vfs for Disk {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn write_durable(&self, path: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
        let temp = temp_name(path);

        let mut temp_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .truncate(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&temp)?;

        use std::io::Write;
        temp_file.write_all(bytes).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })?;

        temp_file.sync_all().inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })?;

        drop(temp_file);

        // Best-effort: preserve the destination's permissions on the temp
        // before it's ever published, so a SWAP publish doesn't change the
        // file's mode.
        if let Ok(metadata) = fs::metadata(path) {
            let _ = fs::set_permissions(&temp, metadata.permissions());
        }

        Ok(temp)
    }

    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        Self::publish(a, b, libc::RENAME_SWAP, "exchange")
    }

    fn rename_excl(&self, old: &Path, new: &Path) -> io::Result<()> {
        Self::publish(old, new, libc::RENAME_EXCL, "renameexcl")
    }

    fn remove(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
            .map_err(|e| io::Error::new(e.kind(), format!("remove {}: {}", path.display(), e)))
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
        })
    }

    /// Canonicalize `path` via `fs::canonicalize`. When the leaf itself
    /// doesn't exist yet (first save of a brand-new file — canonicalize
    /// requires every path component to exist), only the parent directory
    /// is resolved and the unresolved leaf name is re-joined, so a
    /// symlinked parent directory still canonicalizes correctly.
    fn resolve(&self, path: &Path) -> io::Result<PathBuf> {
        if path.exists() {
            return fs::canonicalize(path).map_err(|e| {
                io::Error::new(e.kind(), format!("resolve {}: {}", path.display(), e))
            });
        }
        match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                let canonical_parent = fs::canonicalize(parent).map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("resolve {}: resolve parent: {}", path.display(), e),
                    )
                })?;
                Ok(canonical_parent.join(path.file_name().unwrap_or_default()))
            }
            _ => {
                let canonical_cwd = std::env::current_dir().map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("resolve {}: get current directory: {}", path.display(), e),
                    )
                })?;
                Ok(canonical_cwd.join(path))
            }
        }
    }

    fn mkdir_all(&self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }
}
