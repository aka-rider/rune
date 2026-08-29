use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

pub const MAX_INDEX_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_CORPUS_BYTES: usize = 256 * 1024 * 1024;
pub const READ_BATCH: usize = 64;

const BINARY_EXTENSIONS: [&str; 37] = [
    "mp3", "mp4", "m4a", "mov", "avi", "mkv", "wav", "flac", "ogg", "pdf", "zip", "gz", "tgz",
    "bz2", "xz", "zst", "7z", "rar", "jar", "class", "o", "a", "so", "dylib", "exe", "dll", "wasm",
    "ttf", "otf", "woff", "woff2", "ico", "icns", "db", "sqlite", "bin", "dat",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub path: PathBuf,
    pub display: String,
    pub text: String,
    pub folded: String,
    pub size: u64,
    pub mtime: SystemTime,
}

pub type Fingerprint = (u64, SystemTime);

#[derive(Debug)]
pub enum ReadOutcome {
    Indexed(IndexEntry),
    Unchanged(PathBuf),
    Skipped(PathBuf),
}

pub struct ProjectIndexState {
    pub root: PathBuf,
    pub entries: Vec<Arc<IndexEntry>>,
    pub pending: Vec<(PathBuf, Option<Fingerprint>)>,
    pub build_generation: crate::generation::ProjectIndexGen,
    pub truncated: bool,
    pub building: bool,
    pub last_query: String,
    pub corpus_bytes: usize,
    pub corpus_cap: usize,
    pub spinner_frame: u8,
}

pub(crate) fn is_indexable(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return true;
    };
    let folded = ext.to_ascii_lowercase();
    !BINARY_EXTENSIONS.contains(&folded.as_str())
        && !rune_image::decode::extensions().contains(&folded.as_str())
}
