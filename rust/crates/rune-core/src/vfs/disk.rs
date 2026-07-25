//! Darwin-specific disk-backed `Vfs` using `renamex_np` for atomic saves.
//!
//! Port of Go's `pkg/vfs/exchange_darwin.go` and `pkg/vfs/disk.go`.

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::vfs::Vfs;

/// Disk-backed `Vfs` on Darwin. Uses `renamex_np` for crash-safe atomic
/// saves.
#[derive(Clone, Copy, Default)]
pub struct Disk;

impl Disk {
    fn temp_name(path: &Path) -> PathBuf {
        let parent = path.parent().unwrap_or(Path::new("")).to_path_buf();
        let basename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let pid = std::process::id();
        parent.join(format!(".{basename}.rune-tmp-{pid}"))
    }

    fn fsync_file(file: &File) -> io::Result<()> {
        file.sync_all()
    }

    fn fsync_dir(dir: &Path) -> io::Result<()> {
        let dir_file = File::open(dir)?;
        dir_file.sync_all()
    }

    /// The only `unsafe` block in the entire workspace.
    /// Wraps the Darwin renamex_np syscall to atomically exchange or create files
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
}

impl Vfs for Disk {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn save_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        // Resolve the destination path first to handle symlinks correctly.
        // If path exists (including as a symlink), canonicalize it.
        // Otherwise, canonicalize the parent and join the filename.
        let dest_path = if path.exists() {
            fs::canonicalize(path).map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("failed to resolve path {}: {}", path.display(), e),
                )
            })?
        } else {
            // For non-existent paths, try to canonicalize the parent.
            // For bare relative filenames, the parent may not exist or be empty,
            // so we construct the path manually.
            match path.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => {
                    let canonical_parent = fs::canonicalize(parent).map_err(|e| {
                        io::Error::new(
                            e.kind(),
                            format!("failed to resolve parent of {}: {}", path.display(), e),
                        )
                    })?;
                    canonical_parent.join(path.file_name().unwrap_or_default())
                }
                _ => {
                    // For a bare relative filename (no parent), canonicalize the current directory
                    // and join the filename.
                    let canonical_cwd = std::env::current_dir().map_err(|e| {
                        io::Error::new(
                            e.kind(),
                            format!(
                                "failed to get current directory for relative path {}: {}",
                                path.display(),
                                e
                            ),
                        )
                    })?;
                    canonical_cwd.join(path)
                }
            }
        };

        let temp = Self::temp_name(&dest_path);

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

        Self::fsync_file(&temp_file).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })?;

        drop(temp_file);

        let dest_exists = dest_path.exists();

        if dest_exists {
            // Preserve the destination's permissions before the swap.
            if let Ok(metadata) = fs::metadata(&dest_path) {
                let _ = fs::set_permissions(&temp, metadata.permissions());
            }

            Self::renamex_np(&temp, &dest_path, libc::RENAME_SWAP).inspect_err(|_| {
                let _ = fs::remove_file(&temp);
            })?;

            // After a successful swap, the save has landed. Clean up the temp
            // file best-effort (it holds the old content, now orphaned);
            // don't propagate cleanup errors or skip parent fsync.
            let _ = fs::remove_file(&temp);
        } else {
            Self::renamex_np(&temp, &dest_path, libc::RENAME_EXCL).inspect_err(|_| {
                let _ = fs::remove_file(&temp);
            })?;
        }

        // Always fsync the parent directory, even if temp cleanup failed.
        // The save has landed; directory durability is critical.
        if let Some(parent) = dest_path.parent() {
            let parent_to_fsync = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            Self::fsync_dir(parent_to_fsync).map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!(
                        "failed to fsync parent directory {}: {}",
                        parent_to_fsync.display(),
                        e
                    ),
                )
            })?;
        }

        Ok(())
    }
}
