use std::path::{Path, PathBuf};

use rune_vfs::Vfs;

const MARKER_GIT: &str = ".git";
const MARKER_OBSIDIAN: &str = ".obsidian";

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
        let parent = dir.parent()?;
        dir = parent.to_path_buf();
    }
}

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
    use rune_vfs::{Mem, VfsTestExt};

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

    #[test]
    fn file_hint_retry_fires_only_when_cwd_walk_finds_nothing() {
        let mem = Mem::new();
        mem.save_atomic(Path::new("/home/user/cwdproj/.git/HEAD"), b"x")
            .unwrap();
        mem.save_atomic(Path::new("/mnt/other/.git/HEAD"), b"x")
            .unwrap();

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

    #[test]
    fn file_hint_retry_does_not_fire_when_hint_is_none() {
        let mem = Mem::new();
        mem.save_atomic(Path::new("/mnt/other/.git/HEAD"), b"x")
            .unwrap();

        let root = resolve(&mem, Path::new("/home/user/scratch"), None, None);
        assert_eq!(root, PathBuf::from("/home/user/scratch"));
    }
}
