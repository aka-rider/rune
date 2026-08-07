//! The fuzzy file finder's own Vfs-only, ignore-aware recursive walk:
//! composed entirely from `Vfs::read_dir`/`Vfs::read` rather than the
//! `ignore` crate's own walker, which reads the real filesystem directly
//! and bypasses the injected `Vfs` — fenced off by the workspace's own
//! `disallowed-types` clippy config. Iterative, not recursive: an explicit
//! work stack carries enter/leave steps so a per-directory gitignore
//! matcher can be pushed on entry and popped on leave without recursion
//! depth ever touching the real call stack.

use std::path::{Path, PathBuf};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rune_vfs::{DirEntry, Vfs};

/// Past this many collected files, or this many directory levels below
/// `root`, the walk stops early rather than risking a multi-second `Cmd`
/// against a huge or deeply nested tree — [`ScanResult::truncated`] tells
/// the caller a cap was hit rather than silently under-reporting.
pub const MAX_SCAN_FILES: usize = 10_000;
pub const MAX_SCAN_DEPTH: usize = 32;

/// Directory names skipped outright, in addition to every hidden entry
/// (name starts with `.`) — dependency/build trees nobody fuzzy-opens into,
/// common enough to hardcode rather than wait on a `.gitignore` that may
/// not mention them.
const SKIP_DIRS: [&str; 2] = ["node_modules", "target"];

/// The walk's own result: every file path found, and whether a cap
/// ([`MAX_SCAN_FILES`]/[`MAX_SCAN_DEPTH`]) cut it short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanResult {
    pub files: Vec<PathBuf>,
    pub truncated: bool,
}

/// One step of the explicit DFS work stack: `Enter` reads a directory and
/// pushes its own gitignore matcher before its children are visited;
/// `Leave` pops that matcher once every child (and its whole subtree) has
/// been. Ordinary recursion would do this implicitly on the call stack —
/// keeping an explicit one instead means a workspace nested deeper than any
/// reasonable stack limit still just hits [`MAX_SCAN_DEPTH`].
enum Step {
    Enter(PathBuf, usize),
    Leave,
}

/// Lists every file under `root`, skipping hidden entries, [`SKIP_DIRS`],
/// and anything a gitignore-syntax file marks ignored — any entry whose
/// name starts with `.` and ends with `ignore` (`.gitignore`,
/// `.dockerignore`, …) is read and compiled for its own directory rather
/// than treated as a candidate itself. Ignore files compile into one
/// matcher per directory (`GitignoreBuilder` + `add_line`, never
/// `WalkBuilder`); a line that fails to parse is skipped, never fatal.
/// Nested directories stack their matchers: the deepest directory's own
/// matcher is checked first, and the first definitive verdict (ignore, or a
/// `!`-negated whitelist) wins, so a child `.gitignore` can override a
/// parent's rule exactly like git itself. A symlink to a directory is never
/// descended (`Vfs::read_dir` reports it as a plain file), matching the
/// Explorer's own blindness to it.
pub fn scan(vfs: &dyn Vfs, root: &Path) -> ScanResult {
    let mut files = Vec::new();
    let mut truncated = false;
    let mut matchers: Vec<Gitignore> = Vec::new();
    let mut stack = vec![Step::Enter(root.to_path_buf(), 0)];

    while let Some(step) = stack.pop() {
        let (dir, depth) = match step {
            Step::Leave => {
                matchers.pop();
                continue;
            }
            Step::Enter(dir, depth) => (dir, depth),
        };
        // An unreadable directory (permission denied, vanished mid-walk)
        // drops only its own subtree, never the whole walk.
        let Ok(entries) = vfs.read_dir(&dir) else {
            continue;
        };
        matchers.push(build_matcher(vfs, &dir, &entries));
        stack.push(Step::Leave);

        let mut children = Vec::new();
        for entry in &entries {
            if is_ignore_file(&entry.name) {
                continue; // consumed into the matcher above, never a candidate
            }
            if is_hidden(&entry.name) || is_skiplisted(&entry.name, entry.is_dir) {
                continue;
            }
            if is_ignored(&matchers, &entry.path, entry.is_dir) {
                continue;
            }
            if entry.is_dir {
                if depth + 1 > MAX_SCAN_DEPTH {
                    truncated = true;
                    continue;
                }
                children.push(entry.path.clone());
            } else if files.len() < MAX_SCAN_FILES {
                files.push(entry.path.clone());
            } else {
                truncated = true;
            }
        }
        if files.len() >= MAX_SCAN_FILES {
            truncated = true;
            break;
        }
        // Pushed in reverse so the LIFO stack still visits them in
        // `vfs.read_dir`'s own dirs-first, case-sensitive order.
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

/// Deepest-first, first-verdict-wins: `matchers` is ordered root-to-leaf
/// (the current directory's own matcher was pushed last), so walking it in
/// reverse checks the most specific matcher first. Always `Gitignore::
/// matched`, never `matched_path_or_any_parents` (documented to panic on a
/// path outside its own root).
fn is_ignored(matchers: &[Gitignore], path: &Path, is_dir: bool) -> bool {
    for matcher in matchers.iter().rev() {
        match matcher.matched(path, is_dir) {
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
            Match::None => {}
        }
    }
    false
}

/// Compiles `dir`'s own gitignore-syntax files into one matcher rooted at
/// `dir`. I/O-free by construction: `GitignoreBuilder::new`/`add_line`
/// never touch disk themselves — only this function's own `vfs.read` does,
/// through the injected `Vfs`, never the real filesystem directly. A file
/// that fails to read, isn't valid UTF-8, or contributes a line that fails
/// to parse is skipped rather than aborting the whole directory's matcher.
fn build_matcher(vfs: &dyn Vfs, dir: &Path, entries: &[DirEntry]) -> Gitignore {
    let mut builder = GitignoreBuilder::new(dir);
    for entry in entries {
        if entry.is_dir || !is_ignore_file(&entry.name) {
            continue;
        }
        let Ok(bytes) = vfs.read(&entry.path) else {
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
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
    use rune_vfs::Mem;

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
    fn the_file_cap_truncates_and_reports_it() {
        let vfs = Mem::new();
        for i in 0..(MAX_SCAN_FILES + 5) {
            put(&vfs, &format!("/root/f{i:06}.txt"), "x");
        }

        let result = scan(&vfs, Path::new("/root"));

        assert_eq!(result.files.len(), MAX_SCAN_FILES);
        assert!(result.truncated);
    }

    #[test]
    fn the_depth_cap_truncates_and_reports_it() {
        let vfs = Mem::new();
        let mut deep = String::from("/root");
        for level in 0..(MAX_SCAN_DEPTH + 3) {
            deep.push_str(&format!("/d{level}"));
        }
        put(&vfs, &format!("{deep}/past-cap.txt"), "too deep");
        put(&vfs, "/root/shallow.txt", "within cap");

        let result = scanned(&vfs, "/root");

        assert!(result.files.contains(&PathBuf::from("/root/shallow.txt")));
        assert!(!result.files.iter().any(|f| f.ends_with("past-cap.txt")));
        assert!(result.truncated);
    }
}
