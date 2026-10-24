//! Every byte of a document that reaches the disk, a hash, or the vfs goes
//! through `rune_core::codec`, and every byte that comes back becomes text
//! through it too. A stray `as_bytes()`, `into_bytes()`, or `from_utf8` in
//! one of the document-content modules would silently corrupt files that
//! carry raw bytes, so this walks their production source and fails on any
//! such call that is not on the explicit allowlist below.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::path::{Path, PathBuf};

const GUARDED: &[&str] = &[
    "src/save.rs",
    "src/save",
    "src/materialize_ack.rs",
    "src/materialize_ack",
    "src/rename_create.rs",
    "src/workspace/mod.rs",
    "src/explorer_preview/mod.rs",
    "src/runtime/preview_cmd.rs",
    "src/diff_view/mod.rs",
    "src/merge/landing.rs",
    "src/clipboard.rs",
    "src/dispatch.rs",
    "src/find",
];

const FORBIDDEN: &[&str] = &["from_utf8", ".as_bytes()", ".into_bytes()"];

struct Allowed {
    file: &'static str,
    line_contains: &'static str,
    reason: &'static str,
}

const ALLOWED: &[Allowed] = &[
    Allowed {
        file: "src/merge/landing.rs",
        line_contains: "text.as_bytes()",
        reason: "text-domain bytes of decoded Strings fed to the line differ; never written anywhere",
    },
    Allowed {
        file: "src/merge/landing.rs",
        line_contains: "std::str::from_utf8(",
        reason: "hunks are line slices of valid Strings; re-decoding through the codec would re-escape native reserved codepoints",
    },
    Allowed {
        file: "src/clipboard.rs",
        line_contains: "encoded.as_bytes()",
        reason: "base64 text of the OSC 52 sequence, not document content",
    },
    Allowed {
        file: "src/clipboard.rs",
        line_contains: "rune_core::codec::decode(text.into_bytes())",
        reason: "the one chokepoint that re-reads terminal paste text as disk bytes",
    },
];

fn is_test_only(path: &Path) -> bool {
    path.components().any(|c| {
        let name = c.as_os_str().to_string_lossy();
        name.contains("test") || name.contains("fixture")
    })
}

fn rust_files(root: &Path, entry: &str, out: &mut Vec<PathBuf>) {
    let path = root.join(entry);
    if path.is_file() {
        out.push(path);
        return;
    }
    assert!(path.is_dir(), "guarded path {entry} no longer exists");
    for child in fs::read_dir(&path).unwrap() {
        let child = child.unwrap().path();
        let relative = child
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if child.is_dir() {
            rust_files(root, &relative, out);
        } else if child.extension().is_some_and(|e| e == "rs")
            && !is_test_only(Path::new(&relative))
        {
            out.push(child);
        }
    }
}

fn production_lines(source: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut iter = source.lines().enumerate().peekable();
    while let Some((number, line)) = iter.next() {
        if !line.trim_start().starts_with("#[cfg(test)]") {
            lines.push((number + 1, line));
            continue;
        }
        let mut depth = 0_i32;
        let mut opened = false;
        for (_, item_line) in iter.by_ref() {
            let trimmed = item_line.trim();
            if trimmed.starts_with("#[") {
                continue;
            }
            let opens = i32::try_from(trimmed.matches('{').count()).unwrap();
            let closes = i32::try_from(trimmed.matches('}').count()).unwrap();
            opened |= opens > 0;
            depth += opens - closes;
            if (opened && depth <= 0) || (!opened && trimmed.ends_with(';')) {
                break;
            }
        }
    }
    lines
}

fn code_part(line: &str) -> &str {
    line.split("//").next().unwrap_or(line)
}

#[test]
fn document_content_modules_reach_bytes_only_through_the_codec() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for entry in GUARDED {
        rust_files(root, entry, &mut files);
    }
    assert!(!files.is_empty());

    let mut used = vec![false; ALLOWED.len()];
    let mut violations = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let source = fs::read_to_string(&file).unwrap();
        for (number, line) in production_lines(&source) {
            let code = code_part(line);
            if !FORBIDDEN.iter().any(|f| code.contains(f)) {
                continue;
            }
            let mut allowed = false;
            for (index, entry) in ALLOWED.iter().enumerate() {
                if entry.file == relative && code.contains(entry.line_contains) {
                    used[index] = true;
                    allowed = true;
                }
            }
            if !allowed {
                violations.push(format!("{relative}:{number}: {}", line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "document bytes must go through rune_core::codec; add an allowlist entry only for \
         non-document bytes:\n{}",
        violations.join("\n")
    );
    let stale: Vec<_> = ALLOWED
        .iter()
        .zip(used)
        .filter(|(_, used)| !used)
        .map(|(a, _)| format!("{} {:?} ({})", a.file, a.line_contains, a.reason))
        .collect();
    assert!(
        stale.is_empty(),
        "stale allowlist entries:\n{}",
        stale.join("\n")
    );
}
