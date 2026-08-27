use std::path::{Path, PathBuf};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rune_vfs::{DirEntry, FileKind, Link, Vfs};

pub const MAX_SCAN_FILES: usize = 10_000;
pub const MAX_SCAN_DEPTH: usize = 32;

// A `.gitignore`-family file is a handful of pattern lines, never a
// document — this caps the read well below `rune_vfs::MAX_DOCUMENT_BYTES`
// so a pathological ignore file can't balloon the walk's memory the way an
// unbounded read would.
const MAX_GITIGNORE_BYTES: u64 = 1024 * 1024;

const SKIP_DIRS: [&str; 2] = ["node_modules", "target"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanResult {
    pub files: Vec<PathBuf>,
    pub truncated: bool,
}

enum Step {
    Enter(PathBuf, usize),
    Leave,
}

pub fn scan(vfs: &dyn Vfs, root: &Path) -> ScanResult {
    scan_with_caps(vfs, root, MAX_SCAN_FILES, MAX_SCAN_DEPTH)
}

fn scan_with_caps(vfs: &dyn Vfs, root: &Path, max_files: usize, max_depth: usize) -> ScanResult {
    let mut files = Vec::new();
    let mut truncated = false;
    let mut matchers: Vec<Gitignore> = Vec::new();
    let mut stack = vec![Step::Enter(root.to_path_buf(), 0)];

    'walk: while let Some(step) = stack.pop() {
        let (dir, depth) = match step {
            Step::Leave => {
                matchers.pop();
                continue;
            }
            Step::Enter(dir, depth) => (dir, depth),
        };
        let Ok(entries) = vfs.read_dir(&dir) else {
            continue;
        };
        matchers.push(build_matcher(vfs, &dir, &entries));
        stack.push(Step::Leave);

        let mut children = Vec::new();
        for entry in &entries {
            if entry.link != Link::No {
                continue;
            }
            let entry_is_dir = entry.kind == FileKind::Dir;
            if is_ignore_file(&entry.name) {
                continue;
            }
            if is_hidden(&entry.name) || is_skiplisted(&entry.name, entry_is_dir) {
                continue;
            }
            if is_ignored(&matchers, &entry.path, entry_is_dir) {
                continue;
            }
            if entry_is_dir {
                if depth + 1 > max_depth {
                    truncated = true;
                    continue;
                }
                children.push(entry.path.clone());
            } else if files.len() < max_files {
                files.push(entry.path.clone());
            } else {
                truncated = true;
                break 'walk;
            }
        }
        for child in children.into_iter().rev() {
            stack.push(Step::Enter(child, depth + 1));
        }
    }

    ScanResult { files, truncated }
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

fn is_ignore_file(name: &str) -> bool {
    name.starts_with('.') && name.ends_with("ignore")
}

fn is_skiplisted(name: &str, is_dir: bool) -> bool {
    is_dir && SKIP_DIRS.contains(&name)
}

fn is_ignored(matchers: &[Gitignore], path: &Path, is_dir: bool) -> bool {
    // `Gitignore::matched_path_or_any_parents` is documented to panic on a
    // path outside its own root; `matched` per-matcher (deepest first) avoids it.
    for matcher in matchers.iter().rev() {
        match matcher.matched(path, is_dir) {
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
            Match::None => {}
        }
    }
    false
}

fn build_matcher(vfs: &dyn Vfs, dir: &Path, entries: &[DirEntry]) -> Gitignore {
    let mut builder = GitignoreBuilder::new(dir);
    for entry in entries {
        if entry.kind == FileKind::Dir || !is_ignore_file(&entry.name) {
            continue;
        }
        let Ok(sighting) = rune_vfs::get(vfs, &entry.path, MAX_GITIGNORE_BYTES) else {
            continue;
        };
        let Ok(text) = String::from_utf8(sighting.bytes) else {
            continue;
        };
        for line in text.lines() {
            let _ = builder.add_line(None, line);
        }
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_vfs::{Mem, VfsTestExt};
    use std::fmt::Write as _;

    fn put(vfs: &Mem, path: &str, content: &str) {
        vfs.save_atomic(Path::new(path), content.as_bytes())
            .expect("seed file");
    }

    fn scanned(vfs: &Mem, root: &str) -> ScanResult {
        let mut result = scan(vfs, Path::new(root));
        result.files.sort();
        result
    }

    #[test]
    fn gitignore_pattern_and_negation_are_both_honored() {
        let vfs = Mem::new();
        put(&vfs, "/root/.gitignore", "*.log\n!keep.log\n");
        put(&vfs, "/root/a.log", "excluded");
        put(&vfs, "/root/keep.log", "negated back in");
        put(&vfs, "/root/b.txt", "plain file");

        let result = scanned(&vfs, "/root");

        assert_eq!(
            result.files,
            vec![
                PathBuf::from("/root/b.txt"),
                PathBuf::from("/root/keep.log"),
            ]
        );
        assert!(!result.truncated);
    }

    #[test]
    fn any_dotfile_ending_in_ignore_is_compiled_as_a_gitignore_source() {
        let vfs = Mem::new();
        put(&vfs, "/root/.dockerignore", "secrets.txt\n");
        put(&vfs, "/root/secrets.txt", "excluded");
        put(&vfs, "/root/public.txt", "included");

        let result = scanned(&vfs, "/root");

        assert_eq!(result.files, vec![PathBuf::from("/root/public.txt")]);
    }

    #[test]
    fn a_nested_gitignore_overrides_its_parent() {
        let vfs = Mem::new();
        put(&vfs, "/root/.gitignore", "*.log\n");
        put(&vfs, "/root/sub/.gitignore", "!keep.log\n");
        put(&vfs, "/root/top.log", "excluded by parent");
        put(
            &vfs,
            "/root/sub/a.log",
            "still excluded, only child rule negates keep.log",
        );
        put(
            &vfs,
            "/root/sub/keep.log",
            "negated by the nested gitignore",
        );

        let result = scanned(&vfs, "/root");

        assert_eq!(result.files, vec![PathBuf::from("/root/sub/keep.log")]);
    }

    #[test]
    fn hidden_files_and_directories_are_skipped() {
        let vfs = Mem::new();
        put(&vfs, "/root/.secret", "hidden file");
        put(&vfs, "/root/.hidden/inside.txt", "hidden dir contents");
        put(&vfs, "/root/visible.txt", "kept");

        let result = scanned(&vfs, "/root");

        assert_eq!(result.files, vec![PathBuf::from("/root/visible.txt")]);
    }

    #[test]
    fn node_modules_and_target_directories_are_skipped() {
        let vfs = Mem::new();
        put(&vfs, "/root/node_modules/pkg/index.js", "dep");
        put(&vfs, "/root/target/debug/bin", "build output");
        put(&vfs, "/root/src/main.rs", "kept");

        let result = scanned(&vfs, "/root");

        assert_eq!(result.files, vec![PathBuf::from("/root/src/main.rs")]);
    }

    #[test]
    fn exactly_the_file_cap_is_not_reported_as_truncated() {
        let vfs = Mem::new();
        for i in 0..8 {
            put(&vfs, &format!("/root/f{i:06}.txt"), "x");
        }

        let mut result = scan_with_caps(&vfs, Path::new("/root"), 8, MAX_SCAN_DEPTH);
        result.files.sort();

        assert_eq!(result.files.len(), 8);
        assert!(
            !result.truncated,
            "a walk that finishes exactly at the cap dropped nothing"
        );
    }

    #[test]
    fn one_file_over_the_cap_is_reported_as_truncated() {
        let vfs = Mem::new();
        for i in 0..9 {
            put(&vfs, &format!("/root/f{i:06}.txt"), "x");
        }

        let result = scan_with_caps(&vfs, Path::new("/root"), 8, MAX_SCAN_DEPTH);

        assert_eq!(result.files.len(), 8);
        assert!(result.truncated, "the ninth file was actually dropped");
    }

    #[test]
    fn a_symlinked_directory_is_neither_descended_nor_offered_as_a_candidate() {
        let vfs = Mem::new();
        put(&vfs, "/root/real/inner.md", "reachable by its own path");
        vfs.symlink(Path::new("/root/alias"), Path::new("/root/real"))
            .expect("seed symlink");

        let result = scanned(&vfs, "/root");

        assert_eq!(result.files, vec![PathBuf::from("/root/real/inner.md")]);
        assert!(!result.truncated);
    }

    #[test]
    fn a_directory_symlinked_to_its_own_parent_completes_the_walk() {
        let vfs = Mem::new();
        put(&vfs, "/root/a/real.md", "the only candidate");
        vfs.symlink(Path::new("/root/a/loop"), Path::new("/root/a"))
            .expect("seed self-referential symlink");

        let result = scanned(&vfs, "/root");

        assert_eq!(result.files, vec![PathBuf::from("/root/a/real.md")]);
        assert!(!result.truncated);
    }

    #[test]
    fn a_symlink_to_a_file_is_not_offered_as_a_candidate() {
        let vfs = Mem::new();
        put(&vfs, "/root/real.md", "the only candidate");
        vfs.symlink(Path::new("/root/alias.md"), Path::new("/root/real.md"))
            .expect("seed symlink");

        let result = scanned(&vfs, "/root");

        assert_eq!(result.files, vec![PathBuf::from("/root/real.md")]);
    }

    #[test]
    fn the_depth_cap_truncates_and_reports_it() {
        let vfs = Mem::new();
        let mut deep = String::from("/root");
        for level in 0..(MAX_SCAN_DEPTH + 3) {
            let _ = write!(deep, "/d{level}");
        }
        put(&vfs, &format!("{deep}/past-cap.txt"), "too deep");
        put(&vfs, "/root/shallow.txt", "within cap");

        let result = scanned(&vfs, "/root");

        assert!(result.files.contains(&PathBuf::from("/root/shallow.txt")));
        assert!(!result.files.iter().any(|f| f.ends_with("past-cap.txt")));
        assert!(result.truncated);
    }
}
