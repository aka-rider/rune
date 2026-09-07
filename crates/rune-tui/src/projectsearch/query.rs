use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::index::IndexEntry;
use crate::find::matcher::Matcher;

pub(crate) const MAX_RESULT_FILES: usize = 500;
pub(crate) const MAX_RANGES_PER_FILE: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHit {
    pub path: PathBuf,
    pub display: String,
    pub count: usize,
    pub first_match: usize,
    pub line: u32,
    pub ranges: Vec<Range<usize>>,
}

pub(crate) fn run_query(
    entries: &[Arc<IndexEntry>],
    overrides: &[(PathBuf, String)],
    matcher: &Matcher,
) -> (Vec<FileHit>, bool) {
    let mut hits: Vec<FileHit> = entries
        .iter()
        .filter_map(|entry| {
            let text = override_text(overrides, &entry.path).unwrap_or(entry.text.as_str());
            hit_for(&entry.path, &entry.display, text, matcher)
        })
        .collect();
    hits.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.display.cmp(&b.display))
    });
    let truncated = hits.len() > MAX_RESULT_FILES;
    hits.truncate(MAX_RESULT_FILES);
    (hits, truncated)
}

fn override_text<'a>(overrides: &'a [(PathBuf, String)], path: &Path) -> Option<&'a str> {
    overrides
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, text)| text.as_str())
}

fn hit_for(path: &Path, display: &str, text: &str, matcher: &Matcher) -> Option<FileHit> {
    let mut ranges = matcher.hits(text);
    let count = ranges.len();
    let first_match = ranges.first()?.start;
    let line = line_of(text, first_match);
    ranges.truncate(MAX_RANGES_PER_FILE);
    Some(FileHit {
        path: path.to_path_buf(),
        display: display.to_string(),
        count,
        first_match,
        line,
        ranges,
    })
}

fn line_of(text: &str, offset: usize) -> u32 {
    let newlines = text
        .get(..offset)
        .unwrap_or("")
        .bytes()
        .filter(|&b| b == b'\n')
        .count();
    u32::try_from(newlines)
        .unwrap_or(u32::MAX)
        .saturating_add(1)
}
