//! Resolving a `Target` against the filesystem (500-line budget split of
//! the crate root): the doc-dir-then-root search and the lexical path
//! normalization it depends on. There is no vault-containment restriction —
//! a relative target that lexically escapes `root` is still tried, because
//! the resolver's job is to find the file the user named, not to police
//! where it lives. The `Destination::Url` scheme allowlist is a separate,
//! untouched boundary: it is still the only thing standing between a
//! `Target` and the OS opener.

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
/// crate. Every existence check goes through the injected `Vfs`.
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
        Target::Url(u) => is_external(u).map_or(Destination::Unresolved, Destination::Url),
        // The caller handles same-document anchors without touching the
        // filesystem.
        Target::SameDoc(_) => Destination::Unresolved,
        Target::Path { path, anchor } => {
            resolve_candidate(vfs, path, doc_dir, root, anchor.as_ref(), name_extension)
        }
        Target::Name { name, anchor } => {
            resolve_candidate(vfs, name, doc_dir, root, anchor.as_ref(), name_extension)
        }
    }
}

/// Decode `raw` (infallibly — a malformed escape passes through verbatim,
/// so there is exactly one candidate string, never two), trim it and strip
/// a leading `./`, then search for a regular file in two passes: first the
/// target verbatim, then — only if it is still unresolved and has no
/// extension of its own — the same target with `name_extension` appended.
/// This is the whole of the `Target::Name`/`Target::Path` distinction as
/// far as resolution is concerned: neither ever influences which pass
/// fires, so a link and an image embed naming the same string resolve
/// identically. Each pass tries the candidate as an absolute path directly
/// against the filesystem (a deliberate escape hatch: the user named a
/// location outside the vault explicitly), or, if relative, joined onto
/// `doc_dir` then `root` in turn (locality wins, per the module's
/// contract) and lexically normalized before the `Vfs` ever sees it. There
/// is no requirement that the normalized path stay within `root` — a
/// document whose own directory lies outside the workspace root must still
/// resolve its own relative references.
fn resolve_candidate(
    vfs: &dyn Vfs,
    raw: &str,
    doc_dir: Option<&Path>,
    root: &Path,
    anchor: Option<&Anchor>,
    name_extension: &str,
) -> Destination {
    let decoded = percent::decode(raw);
    let trimmed = decoded.trim();
    let stripped = trimmed.strip_prefix("./").unwrap_or(trimmed);

    if let Some(dest) = try_candidate(vfs, stripped, doc_dir, root, anchor) {
        return dest;
    }
    if Path::new(stripped).extension().is_none() {
        let with_extension = format!("{stripped}.{name_extension}");
        if let Some(dest) = try_candidate(vfs, &with_extension, doc_dir, root, anchor) {
            return dest;
        }
    }
    Destination::Unresolved
}

/// One resolution pass for an already-decoded, already-trimmed candidate
/// string: absolute candidates are checked directly; relative candidates
/// are tried against each non-empty base in turn, doc-dir first.
fn try_candidate(
    vfs: &dyn Vfs,
    candidate: &str,
    doc_dir: Option<&Path>,
    root: &Path,
    anchor: Option<&Anchor>,
) -> Option<Destination> {
    let path = Path::new(candidate);
    if path.is_absolute() {
        return is_regular(vfs, path).then(|| Destination::Location {
            path: path.to_path_buf(),
            anchor: anchor.cloned(),
        });
    }

    for base in [doc_dir, Some(root)].into_iter().flatten() {
        if base.as_os_str().is_empty() {
            continue;
        }
        // Normalized BEFORE it ever reaches the `Vfs`: `Mem` has no real
        // filesystem to collapse `..` for it, so a raw `..`-bearing join
        // would fail to look up even a file that genuinely exists.
        let joined = lexically_normalize(&base.join(candidate));
        if is_regular(vfs, &joined) {
            return Some(Destination::Location {
                path: joined,
                anchor: anchor.cloned(),
            });
        }
    }
    None
}

/// Lexically collapses `.` and `..` components with no filesystem access
/// and no symlink resolution, so it behaves identically whether the
/// injected `Vfs`'s own backing store is an identity (`Mem`) or
/// canonicalizes (`Disk`). This is required, not cosmetic: `Mem` has
/// nothing resembling a real directory tree to collapse `..` against, so a
/// candidate carrying an unresolved `..` component would fail to look up
/// even a file that genuinely exists at the normalized location.
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
#[path = "resolve_tests.rs"]
mod tests;
