//! `rune-nav`: the producer-agnostic navigation vocabulary shared by every
//! kind of jump-to-target — markdown links today, tree-sitter go-to-
//! definition and imports later. Uses (links, embeds, imports) are graph
//! edges; Defs (headings, blocks, symbols) are graph nodes. A future
//! headless vault indexer depends on this crate alone, so it must never
//! depend on `rune-md` or `rune-tui`.

pub mod percent;

use std::path::{Path, PathBuf};

pub use rune_syntax::element::ByteRange;
use rune_vfs::Vfs;

/// A single navigable reference found in a document: where it sits
/// (`site`) and what it is (`kind`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub site: ByteRange,
    pub kind: RefKind,
}

/// A reference is either a USE (an edge pointing somewhere) or a DEF (a
/// node something can point at).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefKind {
    Use { role: UseRole, target: Target },
    Def { role: DefRole, name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseRole {
    Link,
    Embed,
    Import,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefRole {
    Heading(u8),
    Block,
    Symbol,
}

/// What a `Use` points at, before resolution against the filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Url(String),
    Path {
        path: String,
        anchor: Option<Anchor>,
    },
    Name {
        name: String,
        anchor: Option<Anchor>,
    },
    SameDoc(Anchor),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    Heading(String),
    Block(String),
    Line(u32),
}

/// Where a `Target` actually resolves to. `Unresolved` is deliberately a
/// real state, not an error: an unresolvable link is still a graph edge the
/// future vault graph must draw, and the UI reports it rather than hiding
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Url(String),
    Location {
        path: PathBuf,
        anchor: Option<Anchor>,
    },
    Unresolved,
}

/// THIS IS A SECURITY BOUNDARY: it is the allowlist gating a later `open(1)`
/// process spawn, so only these three schemes may ever pass — `file://`,
/// `javascript:`, `data:` and `ftp://` must never be treated as external.
pub fn is_external(raw: &str) -> bool {
    let trimmed = raw.trim().to_ascii_lowercase();
    trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("mailto:")
}

/// Resolve `target` against the filesystem. `doc_dir` is the directory the
/// referencing document lives in (checked before `root`); `root` is the
/// vault/workspace root. Every existence check goes through the injected
/// `Vfs` (§1.4.9).
pub fn resolve(vfs: &dyn Vfs, target: &Target, doc_dir: Option<&Path>, root: &Path) -> Destination {
    match target {
        Target::Url(u) => Destination::Url(u.clone()),
        // The caller handles same-document anchors without touching the
        // filesystem.
        Target::SameDoc(_) => Destination::Unresolved,
        Target::Path { path, anchor } => {
            resolve_candidates(vfs, path, false, doc_dir, root, anchor)
        }
        Target::Name { name, anchor } => resolve_candidates(vfs, name, true, doc_dir, root, anchor),
    }
}

/// Build the candidate list (`[percent::decode(raw), Some(raw)]`,
/// deduplicated, decoded first), process each (trim, strip leading `./`,
/// append `.md` for a `Target::Name` with no extension), and return the
/// first candidate that resolves to a regular file — absolute candidates
/// checked directly, relative ones joined onto `doc_dir` then `root`.
fn resolve_candidates(
    vfs: &dyn Vfs,
    raw: &str,
    is_name: bool,
    doc_dir: Option<&Path>,
    root: &Path,
    anchor: &Option<Anchor>,
) -> Destination {
    let mut raw_candidates: Vec<String> = Vec::new();
    if let Some(decoded) = percent::decode(raw) {
        raw_candidates.push(decoded);
    }
    if !raw_candidates.iter().any(|c| c == raw) {
        raw_candidates.push(raw.to_string());
    }

    let candidates: Vec<String> = raw_candidates
        .into_iter()
        .map(|c| process_candidate(&c, is_name))
        .collect();

    for candidate in &candidates {
        let path = Path::new(candidate);
        if path.is_absolute() {
            if is_regular(vfs, path) {
                return Destination::Location {
                    path: path.to_path_buf(),
                    anchor: anchor.clone(),
                };
            }
            continue;
        }
        for base in [doc_dir, Some(root)].into_iter().flatten() {
            if base.as_os_str().is_empty() {
                continue;
            }
            let joined = base.join(candidate);
            if is_regular(vfs, &joined) {
                return Destination::Location {
                    path: joined,
                    anchor: anchor.clone(),
                };
            }
        }
    }
    Destination::Unresolved
}

fn process_candidate(raw: &str, is_name: bool) -> String {
    let trimmed = raw.trim();
    let stripped = trimmed.strip_prefix("./").unwrap_or(trimmed);
    if is_name && Path::new(stripped).extension().is_none() {
        format!("{stripped}.md")
    } else {
        stripped.to_string()
    }
}

/// This is why WP1 exists: a directory must never resolve as a link
/// target.
fn is_regular(vfs: &dyn Vfs, p: &Path) -> bool {
    matches!(vfs.stat(p), Ok(s) if !s.is_dir)
}

/// Compare an in-document anchor reference against a definition's name
/// after ASCII-lowercasing and collapsing every run of ASCII whitespace to
/// a single space, trimmed both ends.
pub fn anchor_matches(anchor_name: &str, def_name: &str) -> bool {
    normalize_anchor(anchor_name) == normalize_anchor(def_name)
}

fn normalize_anchor(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in s.chars() {
        if c.is_ascii_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(c.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_vfs::Mem;

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
        let dest = resolve(&vfs, &target, None, Path::new("/root"));
        assert_eq!(
            dest,
            Destination::Location {
                path: PathBuf::from("/root/archive/Canary tokens.md"),
                anchor: None,
            }
        );
    }

    #[test]
    fn a_literal_percent_that_is_not_a_valid_escape_resolves_via_the_raw_fallback() {
        let vfs = mem_with(&["/root/100%.md"]);
        let target = path_target("100%.md");
        let dest = resolve(&vfs, &target, None, Path::new("/root"));
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

        let name_dest = resolve(&vfs, &name_target("Setup"), None, Path::new("/root"));
        assert_eq!(
            name_dest,
            Destination::Location {
                path: PathBuf::from("/root/Setup.md"),
                anchor: None,
            }
        );

        // A Path target never gets `.md` appended, so the same bare name
        // does NOT resolve.
        let path_dest = resolve(&vfs, &path_target("Setup"), None, Path::new("/root"));
        assert_eq!(path_dest, Destination::Unresolved);
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
    fn an_absolute_target_that_does_not_exist_is_unresolved() {
        let vfs = Mem::new();
        let target = path_target("/nowhere/ghost.md");
        let dest = resolve(&vfs, &target, None, Path::new("/root"));
        assert_eq!(dest, Destination::Unresolved);
    }

    #[test]
    fn a_directory_target_is_unresolved() {
        // No exact key at `/root/sub`, only a descendant — `Mem` reports it
        // as a synthetic directory (WP1).
        let vfs = mem_with(&["/root/sub/nested.md"]);
        let target = path_target("sub");
        let dest = resolve(&vfs, &target, None, Path::new("/root"));
        assert_eq!(dest, Destination::Unresolved);
    }

    #[test]
    fn same_doc_target_is_unresolved_without_touching_the_filesystem() {
        let vfs = Mem::new();
        let target = Target::SameDoc(Anchor::Heading("setup".to_string()));
        let dest = resolve(&vfs, &target, None, Path::new("/root"));
        assert_eq!(dest, Destination::Unresolved);
    }

    #[test]
    fn url_target_resolves_without_touching_the_filesystem() {
        let vfs = Mem::new();
        let target = Target::Url("https://example.com".to_string());
        let dest = resolve(&vfs, &target, None, Path::new("/root"));
        assert_eq!(dest, Destination::Url("https://example.com".to_string()));
    }

    #[test]
    fn is_external_accepts_the_three_allowed_schemes_case_insensitively() {
        assert!(is_external("http://example.com"));
        assert!(is_external("https://example.com"));
        assert!(is_external("mailto:someone@example.com"));
        assert!(is_external("HTTP://example.com"));
    }

    #[test]
    fn is_external_rejects_every_other_scheme() {
        assert!(!is_external("file:///etc/passwd"));
        assert!(!is_external("javascript:alert(1)"));
        assert!(!is_external("data:text/plain;base64,aGk="));
        assert!(!is_external("ftp://example.com"));
    }

    #[test]
    fn anchor_matches_ignores_case() {
        assert!(anchor_matches("Setup", "setup"));
    }

    #[test]
    fn anchor_matches_collapses_internal_whitespace_runs() {
        assert!(anchor_matches("My  Heading", "my heading"));
    }

    #[test]
    fn anchor_matches_rejects_a_genuine_mismatch() {
        assert!(!anchor_matches("Setup", "Teardown"));
    }
}
