//! Darwin-specific disk-backed `Vfs` using `renamex_np` for atomic saves.
//!
//! Port of Go's `pkg/vfs/exchange_darwin.go` and `pkg/vfs/disk.go`.

use crate::vfs::Vfs;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

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
    fn renamex_np(src: &Path, dst: &Path, flags: libc::c_uint) -> io::Result<()> {
        let src_c = CString::new(src.to_string_lossy().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let dst_c = CString::new(dst.to_string_lossy().as_bytes())
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
        let temp = Self::temp_name(path);

        let mut temp_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .truncate(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(&temp)
            .inspect_err(|_| {
                let _ = fs::remove_file(&temp);
            })?;

        use std::io::Write;
        temp_file.write_all(bytes).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })?;

        Self::fsync_file(&temp_file).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })?;

        drop(temp_file);

        let dest_exists = path.exists();

        if dest_exists {
            Self::renamex_np(&temp, path, libc::RENAME_SWAP).inspect_err(|_| {
                let _ = fs::remove_file(&temp);
            })?;
            fs::remove_file(&temp).map_err(|e| {
                eprintln!("warning: swap residue cleanup failed: {e}");
                e
            })?;
        } else {
            Self::renamex_np(&temp, path, libc::RENAME_EXCL).inspect_err(|_| {
                let _ = fs::remove_file(&temp);
            })?;
        }

        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            Self::fsync_dir(parent).map_err(|e| {
                eprintln!("warning: dir fsync failed: {e}");
                e
            })?;
        }

        Ok(())
    }
}
