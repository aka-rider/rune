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
fn an_extension_less_target_is_tried_bare_first_then_with_extension_appended() {
    // Only `Setup.md` exists, so the bare-first pass misses and the
    // retry with `name_extension` appended is what resolves it. This
    // holds identically for `Target::Name` and `Target::Path` — the
    // two-pass policy is uniform, so links and image embeds naming the
    // same string can never diverge.
    let vfs = mem_with(&["/root/Setup.md"]);

    let name_dest = resolve(&vfs, &name_target("Setup"), None, Path::new("/root"), MD);
    assert_eq!(
        name_dest,
        Destination::Location {
            path: PathBuf::from("/root/Setup.md"),
            anchor: None,
        }
    );

    let path_dest = resolve(&vfs, &path_target("Setup"), None, Path::new("/root"), MD);
    assert_eq!(
        path_dest,
        Destination::Location {
            path: PathBuf::from("/root/Setup.md"),
            anchor: None,
        }
    );
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
    // An absolute path is the user explicitly naming a location outside
    // the vault, a deliberate escape hatch — and, since there is no
    // containment restriction on relative targets either, this is now
    // no different in kind from any other resolution.
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
fn a_percent_encoded_relative_escape_above_root_resolves_if_the_file_exists() {
    // There is no vault-containment restriction: the decoded
    // `../../etc/hosts` candidate lexically escapes `/root`, but it is
    // still tried, and resolves because the file exists.
    let vfs = mem_with(&["/etc/hosts"]);
    let target = path_target("%2e%2e/%2e%2e/etc/hosts");
    let dest = resolve(&vfs, &target, None, Path::new("/root"), MD);
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/etc/hosts"),
            anchor: None,
        }
    );
}

#[test]
fn a_relative_escape_through_doc_dir_above_root_resolves_if_the_file_exists() {
    let vfs = mem_with(&["/etc/hosts"]);
    let target = path_target("../../../etc/hosts");
    let dest = resolve(
        &vfs,
        &target,
        Some(Path::new("/root/a/b")),
        Path::new("/root"),
        MD,
    );
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/etc/hosts"),
            anchor: None,
        }
    );
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
fn a_relative_target_resolves_against_a_doc_dir_that_lies_outside_root() {
    // The reported bug: opening a document whose own directory sits
    // outside the workspace root must not break resolution of its
    // relative references.
    let vfs = mem_with(&["/elsewhere/assets/x.png"]);
    let target = path_target("assets/x.png");
    let dest = resolve(
        &vfs,
        &target,
        Some(Path::new("/elsewhere")),
        Path::new("/root"),
        MD,
    );
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/elsewhere/assets/x.png"),
            anchor: None,
        }
    );
}

#[test]
fn a_bare_basename_resolves_against_a_doc_dir_outside_root() {
    // The exact reported case, `![[Do not try to DRY.webp]]`: a target
    // with no directory component at all, resolved against a doc_dir
    // outside root. A subdirectory form would mask which rule (bare
    // basename vs. containment) was actually at fault.
    let vfs = mem_with(&["/elsewhere/Do not try to DRY.webp"]);
    let target = path_target("Do not try to DRY.webp");
    let dest = resolve(
        &vfs,
        &target,
        Some(Path::new("/elsewhere")),
        Path::new("/root"),
        MD,
    );
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/elsewhere/Do not try to DRY.webp"),
            anchor: None,
        }
    );
}

#[test]
fn an_extension_less_target_prefers_a_real_extension_less_file_over_the_same_named_md() {
    let vfs = mem_with(&["/root/note", "/root/note.md"]);
    let dest = resolve(&vfs, &name_target("note"), None, Path::new("/root"), MD);
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/root/note"),
            anchor: None,
        }
    );
}

#[test]
fn a_target_containing_spaces_resolves() {
    let vfs = mem_with(&["/root/my notes/weekly review.md"]);
    let target = path_target("my notes/weekly review.md");
    let dest = resolve(&vfs, &target, None, Path::new("/root"), MD);
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/root/my notes/weekly review.md"),
            anchor: None,
        }
    );
}

#[test]
fn a_parent_dir_sibling_form_resolves() {
    let vfs = mem_with(&["/root/sibling/x.png"]);
    let target = path_target("../sibling/x.png");
    let dest = resolve(
        &vfs,
        &target,
        Some(Path::new("/root/current")),
        Path::new("/root"),
        MD,
    );
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/root/sibling/x.png"),
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
