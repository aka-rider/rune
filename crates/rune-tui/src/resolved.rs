use std::path::{Path, PathBuf};

use rune_vfs::Vfs;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResolvedPath(PathBuf);

impl ResolvedPath {
    pub fn resolve(vfs: &dyn Vfs, path: &Path) -> std::io::Result<ResolvedPath> {
        vfs.resolve(path).map(ResolvedPath)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl std::ops::Deref for ResolvedPath {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for ResolvedPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}
