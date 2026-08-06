use std::fs;
use std::path::PathBuf;

use crate::hash::fnv1a32;

pub(crate) fn must<T, E: std::fmt::Debug>(r: Result<T, E>, what: &str) -> T {
    match r {
        Ok(v) => v,
        Err(e) => unreachable!("{what} failed: {e:?}"),
    }
}

pub(crate) struct ScratchDir(PathBuf);

impl ScratchDir {
    pub(crate) fn new(label: &str) -> ScratchDir {
        let dir = std::env::temp_dir().join(format!(
            "rune-fuzz-{label}-{:08x}-{}",
            fnv1a32(label.as_bytes()),
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        ScratchDir(dir)
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
