//! Silent workspace-root discovery: walks up looking for a
//! marker, with no consent prompt. The recovery store lives entirely
//! outside the user's tree (`production_db_path()`), so there is nothing to
//! create and nothing to consent to — this module only ever answers "where
//! is the nearest project/vault root", never prompts, and can never fail
//! the app (a `read_dir` error just means "no markers here", not a halt).

use std::path::{Path, PathBuf};

use rune_vfs::Vfs;

/// Marker names scanned for at each directory level, matched BY NAME ONLY.
/// A git worktree or submodule's `.git` is a *file*, not a directory, so
/// this never tests `is_dir`.
const MARKER_GIT: &str = ".git";
const MARKER_OBSIDIAN: &str = ".obsidian";

/// Resolves the workspace root by walking up from `cwd` (and, if that finds
/// nothing, from `file_hint`'s parent) looking for a `.git`/`.obsidian`
/// marker. Returns the nearest ancestor directory holding a marker, or
/// `cwd` itself when neither walk finds one.
///
/// Exactly one `vfs.read_dir` call is made per directory level visited on
/// each walk — a `read_dir` error is treated as "no markers here" and the
/// walk keeps climbing rather than stopping (discovery must never halt the
/// app).
pub fn resolve(
    vfs: &dyn Vfs,
    cwd: &Path,
    home: Option<&Path>,
    file_hint: Option<&Path>,
) -> PathBuf {
    if let Some(root) = climb(vfs, cwd, home) {
        return root;
    }
    if let Some(hint) = file_hint
        && let Some(start) = hint.parent()
        && let Some(root) = climb(vfs, start, home)
    {
        return root;
    }
    cwd.to_path_buf()
}

/// One bottom-up walk from `start` to its ceiling (S2: `home` inclusive when
/// `start` is under `home`, otherwise `/`), returning the nearest ancestor
/// (including `start` itself) that carries a `.git`/`.obsidian` marker.
fn climb(vfs: &dyn Vfs, start: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let ceiling = ceiling_for(start, home);

    let mut dir = start.to_path_buf();
    loop {
        if let Ok(entries) = vfs.read_dir(&dir)
            && entries
                .iter()
                .any(|e| e.name == MARKER_GIT || e.name == MARKER_OBSIDIAN)
        {
            return Some(dir);
        }

        if dir == ceiling {
            return None;
        }
        // Reached the filesystem root without hitting the ceiling.
        let parent = dir.parent()?;
        dir = parent.to_path_buf();
    }
}

/// S2's ceiling rule: if `start` is `home` or a descendant of it, the climb
/// stops at `home` inclusive; otherwise it climbs all the way to `/`. An
/// absent `home` behaves like "not under home" — the ceiling is `/`.
fn ceiling_for(start: &Path, home: Option<&Path>) -> PathBuf {
    match home {
        Some(home) if start.starts_with(home) => home.to_path_buf(),
        _ => PathBuf::from("/"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_vfs::Mem;

    #[test]
    fn marker_in_cwd_is_found_immediately() {
        let mem = Mem::new();
        mem.save_atomic(Path::new("/home/user/proj/.git/HEAD"), b"ref: x")
            .unwrap();
        let root = resolve(&mem, Path::new("/home/user/proj"), None, None);
        assert_eq!(root, PathBuf::from("/home/user/proj"));
    }

    #[test]
    fn marker_in_an_ancestor_is_found_while_climbing() {
        let mem = Mem::new();
        mem.save_atomic(Path::new("/home/user/proj/.git/HEAD"), b"ref: x")
            .unwrap();
        let root = resolve(&mem, Path::new("/home/user/proj/src/nested"), None, None);
        assert_eq!(root, PathBuf::from("/home/user/proj"));
    }

    /// A git worktree/submodule's `.git` is a FILE, not a directory —
    /// `save_atomic` here writes `.git` itself as a file (not a path
    /// underneath it), and the marker must still be found by name alone.
    #[test]
    fn git_marker_as_a_file_not_a_directory_still_counts() {
        let mem = Mem::new();
        mem.save_atomic(
            Path::new("/home/user/proj/.git"),
            b"gitdir: ../other/.git/worktrees/proj",
        )
        .unwrap();
        let root = resolve(&mem, Path::new("/home/user/proj"), None, None);
        assert_eq!(root, PathBuf::from("/home/user/proj"));
    }

    #[test]
    fn no_marker_anywhere_returns_cwd() {
        let mem = Mem::new();
        mem.save_atomic(Path::new("/home/user/scratch/note.md"), b"hi")
            .unwrap();
        let root = resolve(&mem, Path::new("/home/user/scratch"), None, None);
        assert_eq!(root, PathBuf::from("/home/user/scratch"));
    }

    /// A marker sitting above `home` must never be found: the climb stops
    /// at `home` inclusive.
    #[test]
    fn the_home_ceiling_stops_the_climb() {
        let mem = Mem::new();
        mem.save_atomic(Path::new("/.git/HEAD"), b"ref: x").unwrap();
        let root = resolve(
            &mem,
            Path::new("/home/user/proj"),
            Some(Path::new("/home/user")),
            None,
        );
        assert_eq!(root, PathBuf::from("/home/user/proj"));
    }

    /// `cwd` outside `home` entirely climbs all the way to `/` instead of
    /// stopping at (or being bounded by) `home`.
    #[test]
    fn cwd_outside_home_climbs_to_root() {
        let mem = Mem::new();
        mem.save_atomic(Path::new("/mnt/vault/.obsidian/app.json"), b"{}")
            .unwrap();
        let root = resolve(
            &mem,
            Path::new("/mnt/vault/notes"),
            Some(Path::new("/home/user")),
            None,
        );
        assert_eq!(root, PathBuf::from("/mnt/vault"));
    }

    /// The `file_hint` retry only fires when the `cwd` walk found nothing.
    #[test]
    fn file_hint_retry_fires_only_when_cwd_walk_finds_nothing() {
        let mem = Mem::new();
        mem.save_atomic(Path::new("/home/user/cwdproj/.git/HEAD"), b"x")
            .unwrap();
        mem.save_atomic(Path::new("/mnt/other/.git/HEAD"), b"x")
            .unwrap();

        // cwd walk finds its own marker first — the hint's project must be
        // ignored even though it exists.
        let root = resolve(
            &mem,
            Path::new("/home/user/cwdproj"),
            None,
            Some(Path::new("/mnt/other/notes/note.md")),
        );
        assert_eq!(root, PathBuf::from("/home/user/cwdproj"));
    }

    #[test]
    fn file_hint_retry_fires_when_cwd_walk_finds_nothing() {
        let mem = Mem::new();
        mem.save_atomic(Path::new("/mnt/other/.git/HEAD"), b"x")
            .unwrap();

        let root = resolve(
            &mem,
            Path::new("/home/user/scratch"),
            None,
            Some(Path::new("/mnt/other/notes/note.md")),
        );
        assert_eq!(root, PathBuf::from("/mnt/other"));
    }

    /// The retry must not fire at all when `file_hint` is `None` — the walk
    /// just falls back to `cwd`.
    #[test]
    fn file_hint_retry_does_not_fire_when_hint_is_none() {
        let mem = Mem::new();
        mem.save_atomic(Path::new("/mnt/other/.git/HEAD"), b"x")
            .unwrap();

        let root = resolve(&mem, Path::new("/home/user/scratch"), None, None);
        assert_eq!(root, PathBuf::from("/home/user/scratch"));
    }
}
