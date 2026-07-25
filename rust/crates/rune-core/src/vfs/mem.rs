//! In-memory `Vfs` for tests — mirrors Go's `pkg/vfs/mem.go`.
//!
//! A `Mutex`-backed `HashMap<PathBuf, Vec<u8>>` with an optional
//! `fail_next_save` hook for testing error paths.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::vfs::Vfs;

/// In-memory `Vfs` keyed by `PathBuf`. Suitable for tests.
pub struct Mem {
    data: Mutex<HashMap<PathBuf, Vec<u8>>>,
    fail_next: Mutex<Option<io::Error>>,
}

impl Mem {
    pub fn new() -> Self {
        Mem {
            data: Mutex::new(HashMap::new()),
            fail_next: Mutex::new(None),
        }
    }

    /// Set the next `save_atomic` to fail with the given error kind.
    ///
    /// The failure fires exactly once and is then cleared.
    pub fn fail_next_save(&self, kind: io::ErrorKind) {
        let err = io::Error::new(kind, "fail_next_save triggered");
        let mut guard = self.fail_next.lock().unwrap_or_else(|p| p.into_inner());
        *guard = Some(err);
    }

    fn lock_data(&self) -> std::io::Result<std::sync::MutexGuard<'_, HashMap<PathBuf, Vec<u8>>>> {
        Ok(self.data.lock().unwrap_or_else(|p| p.into_inner()))
    }

    fn lock_fail(&self) -> std::io::Result<std::sync::MutexGuard<'_, Option<io::Error>>> {
        Ok(self.fail_next.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

impl Default for Mem {
    fn default() -> Self {
        Self::new()
    }
}

impl Vfs for Mem {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let data = self.lock_data()?;
        data.get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "file not found in mem vfs"))
    }

    fn save_atomic(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        // Check and consume the fail-next flag before any mutation.
        {
            let mut fail = self.lock_fail()?;
            if let Some(err) = fail.take() {
                return Err(err);
            }
        }
        let mut data = self.lock_data()?;
        data.insert(path.to_path_buf(), bytes.to_vec());
        Ok(())
    }
}
