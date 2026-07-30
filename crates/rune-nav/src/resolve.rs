//! Resolving a `Target` against the filesystem (CONSTITUTION §1.6 split of
//! the crate root): the doc-dir-then-root search, the vault-containment
//! check, and the lexical path normalization it depends on.

use std::path::{Component, Path, PathBuf};

use rune_vfs::{FileKind, Vfs};

use crate::external::is_external;
use crate::percent;
use crate::types::{Anchor, Destination, Target};

/// Resolve `target` against the filesystem. `doc_dir` is the directory the
/// referencing document lives in (checked before `root`); `root` is the
/// vault/workspace root. `name_extension` is the file extension (without a
/// leading dot, e.g. `"md"`) a `Target::Name` candidate gets when it has
/// none of its own — supplied by the producer's resolution policy (e.g.
/// rune-md's catalogue), never hardcoded here, so a non-markdown producer
/// resolving symbol names against `.py` files needs no change to this
/// crate. Every existence check goes through the injected `Vfs` (§1.4.9).
pub fn resolve(
    vfs: &dyn Vfs,
    target: &Target,
    doc_dir: Option<&Path>,
    root: &Path,
    name_extension: &str,
) -> Destination {
    match target {
        // The allowlist is re-checked HERE, not trusted from whichever
        // producer classified the target. `Destination::Url` is the only
        // value that reaches the OS opener's process spawn, so the scheme
        // check belongs at this boundary — a producer added later (a
        // tree-sitter language, a vault indexer) then cannot smuggle a
        // `javascript:`/`file://` target through to it, however it builds
        // its `Target`s.
        Target::Url(u) => match is_external(u) {
            Some(approved) => Destination::Url(approved),
            None => Destination::Unresolved,
        },
        // The caller handles same-document anchors without touching the
        // filesystem.
        Target::SameDoc(_) => Destination::Unresolved,
        Target::Path { path, anchor } => {
            resolve_candidate(vfs, path, false, doc_dir, root, anchor, name_extension)
        }
        Target::Name { name, anchor } => {
            resolve_candidate(vfs, name, true, doc_dir, root, anchor, name_extension)
        }
    }
}

/// Decode `raw` (infallibly — a malformed escape passes through verbatim,
/// so there is exactly one candidate string, never two), process it (trim,
/// strip a leading `./`, append `name_extension` for an extension-less
/// `Target::Name`), and return the first location that resolves to a
/// regular file: an absolute candidate is checked directly against the
/// filesystem, with NO vault-containment check — an absolute path is the
/// user explicitly naming a location outside the vault, a deliberate
/// escape hatch, not the accidental one the relative branch below closes.
/// A relative candidate is joined onto `doc_dir` then `root` (locality
/// wins, per the module's contract), lexically normalized, and must lie
/// within `root` or it is skipped, never even checked against the `Vfs`.
fn resolve_candidate(
    vfs: &dyn Vfs,
    raw: &str,
    is_name: bool,
    doc_dir: Option<&Path>,
    root: &Path,
    anchor: &Option<Anchor>,
    name_extension: &str,
) -> Destination {
    let decoded = percent::decode(raw);
    let candidate = process_candidate(&decoded, is_name, name_extension);
    let path = Path::new(&candidate);

    if path.is_absolute() {
        return if is_regular(vfs, path) {
            Destination::Location {
                path: path.to_path_buf(),
                anchor: anchor.clone(),
            }
        } else {
            Destination::Unresolved
        };
    }

    for base in [doc_dir, Some(root)].into_iter().flatten() {
        if base.as_os_str().is_empty() {
            continue;
        }
        // Normalize BEFORE the containment check and before it ever reaches
        // the `Vfs` — checking containment against the raw, `..`-bearing
        // join and then stat-ing (or returning) that same raw join would
        // both defeat the check (a raw `/root/sub/../../etc/hosts` string
        // literally starts with `/root/sub`) and hand the `Vfs` a path it
        // can't look up (`Mem` has no filesystem to collapse `..` for it).
        let joined = lexically_normalize(&base.join(&candidate));
        if !joined.starts_with(root) {
            continue;
        }
        if is_regular(vfs, &joined) {
            return Destination::Location {
                path: joined,
                anchor: anchor.clone(),
            };
        }
    }
    Destination::Unresolved
}

fn process_candidate(raw: &str, is_name: bool, name_extension: &str) -> String {
    let trimmed = raw.trim();
    let stripped = trimmed.strip_prefix("./").unwrap_or(trimmed);
    if is_name && Path::new(stripped).extension().is_none() {
        format!("{stripped}.{name_extension}")
    } else {
        stripped.to_string()
    }
}

/// Lexically collapses `.` and `..` components with no filesystem access
/// and no symlink resolution, so it behaves identically whether the
/// injected `Vfs`'s own `resolve` is an identity (`Mem`) or canonicalizes
/// (`Disk`) — the vault-root containment check (plan Assumption A2) needs
/// this collapse to happen BEFORE the `starts_with(root)` comparison,
/// since a leading `..` cannot pop past the path's own root component: a
/// decoded `../../../etc/hosts` candidate joined onto `root` normalizes to
/// somewhere outside `root` and so is rejected by the caller, before it is
/// ever stat'd.
fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Only a regular file resolves as a link target. A directory cannot be
/// opened as a buffer, and a FIFO, socket or device node is worse than
/// useless: the open path reads synchronously, so following a link to one
/// would block the editor forever with the buffer unsaved.
fn is_regular(vfs: &dyn Vfs, p: &Path) -> bool {
    matches!(vfs.stat(p), Ok(s) if s.kind == FileKind::File)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::types::AnchorRole;
    use rune_vfs::Mem;

    const MD: &str = "md";

    fn mem_with(paths: &[&str]) -> Mem {
        let vfs = Mem::new();
        for p in paths {
            vfs.save_atomic(&PathBuf::from(p), b"content")
                .expect("seed file");
        }
        vfs
    }

    fn path_target(path: &str) -> Target {
        Target::Path {
            path: path.to_string(),
            anchor: None,
        }
    }

    fn name_target(name: &str) -> Target {
        Target::Name {
            name: name.to_string(),
            anchor: None,
        }
    }

    #[test]
    fn percent_decoded_target_resolves_to_the_percent_containing_file() {
        let vfs = mem_with(&["/root/archive/Canary tokens.md"]);
        let target = path_target("archive/Canary%20tokens.md");
        let dest = resolve(&vfs, &target, None, Path::new("/root"), MD);
        assert_eq!(
            dest,
            Destination::Location {
                path: PathBuf::from("/root/archive/Canary tokens.md"),
                anchor: None,
            }
        );
    }

    #[test]
    fn a_literal_percent_that_is_not_a_valid_escape_resolves_via_the_verbatim_passthrough() {
        let vfs = mem_with(&["/root/100%.md"]);
        let target = path_target("100%.md");
        let dest = resolve(&vfs, &target, None, Path::new("/root"), MD);
        assert_eq!(
            dest,
            Destination::Location {
                path: PathBuf::from("/root/100%.md"),
                anchor: None,
            }
        );
    }

    #[test]
    fn md_extension_is_appended_only_for_name_targets() {
        let vfs = mem_with(&["/root/Setup.md"]);

        let name_dest = resolve(&vfs, &name_target("Setup"), None, Path::new("/root"), MD);
        assert_eq!(
            name_dest,
            Destination::Location {
                path: PathBuf::from("/root/Setup.md"),
                anchor: None,
            }
        );

        // A Path target never gets an extension appended, so the same bare
        // name does NOT resolve.
        let path_dest = resolve(&vfs, &path_target("Setup"), None, Path::new("/root"), MD);
        assert_eq!(path_dest, Destination::Unresolved);
    }

    #[test]
    fn name_extension_is_a_caller_supplied_policy_not_a_hardcoded_choice() {
        let vfs = mem_with(&["/root/utils.py"]);
        let dest = resolve(&vfs, &name_target("utils"), None, Path::new("/root"), "py");
        assert_eq!(
            dest,
            Destination::Location {
                path: PathBuf::from("/root/utils.py"),
                anchor: None,
            }
        );
    }

    #[test]
    fn a_name_target_that_already_has_an_extension_gets_no_second_one_appended() {
        let vfs = mem_with(&["/root/notes.txt"]);
        let dest = resolve(
            &vfs,
            &name_target("notes.txt"),
            None,
            Path::new("/root"),
            MD,
        );
        assert_eq!(
            dest,
            Destination::Location {
                path: PathBuf::from("/root/notes.txt"),
                anchor: None,
            }
        );
    }

    #[test]
    fn doc_dir_wins_over_root_when_both_contain_the_name() {
        let vfs = mem_with(&["/root/note.md", "/root/sub/note.md"]);
        let target = path_target("note.md");
        let dest = resolve(
            &vfs,
            &target,
            Some(Path::new("/root/sub")),
            Path::new("/root"),
            MD,
        );
        assert_eq!(
            dest,
            Destination::Location {
                path: PathBuf::from("/root/sub/note.md"),
                anchor: None,
            }
        );
    }

    #[test]
    fn an_empty_doc_dir_is_skipped_and_root_is_still_tried() {
        let vfs = mem_with(&["/root/note.md"]);
        let target = path_target("note.md");
        let dest = resolve(&vfs, &target, Some(Path::new("")), Path::new("/root"), MD);
        assert_eq!(
            dest,
            Destination::Location {
                path: PathBuf::from("/root/note.md"),
                anchor: None,
            }
        );
    }

    #[test]
    fn an_absolute_target_that_does_not_exist_is_unresolved() {
        let vfs = Mem::new();
        let target = path_target("/nowhere/ghost.md");
        let dest = resolve(&vfs, &target, None, Path::new("/root"), MD);
        assert_eq!(dest, Destination::Unresolved);
    }

    #[test]
    fn an_absolute_target_outside_the_vault_root_still_resolves_deliberately() {
        // Assumption A2: the absolute-path branch is a documented escape
        // hatch, not subject to the containment check below.
        let vfs = mem_with(&["/elsewhere/ghost.md"]);
        let target = path_target("/elsewhere/ghost.md");
        let dest = resolve(&vfs, &target, None, Path::new("/root"), MD);
        assert_eq!(
            dest,
            Destination::Location {
                path: PathBuf::from("/elsewhere/ghost.md"),
                anchor: None,
            }
        );
    }

    #[test]
    fn a_directory_target_is_unresolved() {
        // No exact key at `/root/sub`, only a descendant — `Mem` reports it
        // as a synthetic directory (WP1).
        let vfs = mem_with(&["/root/sub/nested.md"]);
        let target = path_target("sub");
        let dest = resolve(&vfs, &target, None, Path::new("/root"), MD);
        assert_eq!(dest, Destination::Unresolved);
    }

    #[test]
    fn a_percent_encoded_relative_escape_above_root_is_rejected() {
        // The crate's own containment policy (A2): even though
        // `/etc/hosts` exists in this Mem, the decoded `../../etc/hosts`
        // candidate lexically escapes `/root` and must never be tried.
        let vfs = mem_with(&["/etc/hosts"]);
        let target = path_target("%2e%2e/%2e%2e/etc/hosts");
        let dest = resolve(&vfs, &target, None, Path::new("/root"), MD);
        assert_eq!(dest, Destination::Unresolved);
    }

    #[test]
    fn a_relative_escape_through_doc_dir_above_root_is_rejected() {
        let vfs = mem_with(&["/etc/hosts"]);
        let target = path_target("../../../etc/hosts");
        let dest = resolve(
            &vfs,
            &target,
            Some(Path::new("/root/a/b")),
            Path::new("/root"),
            MD,
        );
        assert_eq!(dest, Destination::Unresolved);
    }

    #[test]
    fn a_relative_traversal_that_stays_inside_root_still_resolves() {
        let vfs = mem_with(&["/root/note.md"]);
        let target = path_target("../note.md");
        let dest = resolve(
            &vfs,
            &target,
            Some(Path::new("/root/sub")),
            Path::new("/root"),
            MD,
        );
        assert_eq!(
            dest,
            Destination::Location {
                path: PathBuf::from("/root/note.md"),
                anchor: None,
            }
        );
    }

    #[test]
    fn same_doc_target_is_unresolved_without_touching_the_filesystem() {
        let vfs = Mem::new();
        let target = Target::SameDoc(Anchor::Named {
            role: AnchorRole::Heading,
            name: "setup".to_string(),
        });
        let dest = resolve(&vfs, &target, None, Path::new("/root"), MD);
        assert_eq!(dest, Destination::Unresolved);
    }

    #[test]
    fn url_target_resolves_without_touching_the_filesystem() {
        let vfs = Mem::new();
        let target = Target::Url("https://example.com".to_string());
        let dest = resolve(&vfs, &target, None, Path::new("/root"), MD);
        assert_eq!(dest, Destination::Url("https://example.com".to_string()));
    }

    #[test]
    fn resolve_returns_the_is_external_approved_value_not_the_raw_target() {
        let vfs = Mem::new();
        let target = Target::Url("  HTTPS://Example.com  ".to_string());
        let dest = resolve(&vfs, &target, None, Path::new("/root"), MD);
        assert_eq!(dest, Destination::Url("HTTPS://Example.com".to_string()));
    }

    /// The allowlist is a property of `resolve` itself, not of whichever
    /// producer built the `Target` — a producer added later must not be able
    /// to reach the OS opener with a non-allowlisted scheme.
    #[test]
    fn a_non_allowlisted_url_target_never_becomes_a_url_destination() {
        let vfs = Mem::new();
        for hostile in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/plain;base64,aGk=",
            "ftp://example.com",
        ] {
            let target = Target::Url(hostile.to_string());
            assert_eq!(
                resolve(&vfs, &target, None, Path::new("/root"), MD),
                Destination::Unresolved,
                "{hostile} must not resolve to a Url destination"
            );
        }
    }
}
