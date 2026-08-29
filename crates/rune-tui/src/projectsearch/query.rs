use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::index::IndexEntry;

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

enum Case {
    Sensitive,
    Folded(String),
}

pub(crate) fn run_query(
    entries: &[Arc<IndexEntry>],
    overrides: &[(PathBuf, String)],
    query: &str,
) -> (Vec<FileHit>, bool) {
    if query.is_empty() {
        return (Vec::new(), false);
    }
    let case = if query.chars().any(char::is_uppercase) {
        Case::Sensitive
    } else {
        Case::Folded(query.chars().flat_map(char::to_lowercase).collect())
    };
    let mut hits: Vec<FileHit> = entries
        .iter()
        .filter_map(|entry| {
            let (text, stored_folded) = match override_text(overrides, &entry.path) {
                Some(text) => (text, None),
                None => (entry.text.as_str(), Some(entry.folded.as_str())),
            };
            hit_for(
                &entry.path,
                &entry.display,
                text,
                stored_folded,
                query,
                &case,
            )
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

fn hit_for(
    path: &Path,
    display: &str,
    text: &str,
    stored_folded: Option<&str>,
    query: &str,
    case: &Case,
) -> Option<FileHit> {
    let mut ranges = ranges_in(text, stored_folded, query, case);
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

fn ranges_in(
    text: &str,
    stored_folded: Option<&str>,
    query: &str,
    case: &Case,
) -> Vec<Range<usize>> {
    match case {
        Case::Sensitive => text
            .match_indices(query)
            .map(|(start, matched)| start..start + matched.len())
            .collect(),
        Case::Folded(folded_query) => {
            if folded_query.is_empty() {
                return Vec::new();
            }
            match stored_folded {
                Some(folded) => {
                    if !folded.contains(folded_query.as_str()) {
                        return Vec::new();
                    }
                    let (_, map) = crate::search::fold_with_map(text);
                    translate(folded, folded_query, &map)
                }
                None => {
                    let (folded, map) = crate::search::fold_with_map(text);
                    translate(&folded, folded_query, &map)
                }
            }
        }
    }
}

fn translate(folded: &str, folded_query: &str, map: &[Range<usize>]) -> Vec<Range<usize>> {
    folded
        .match_indices(folded_query)
        .filter_map(|(s, matched)| {
            let e = s + matched.len();
            let start = map.get(s)?.start;
            let end = map.get(e - 1)?.end;
            Some(start..end)
        })
        .collect()
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
