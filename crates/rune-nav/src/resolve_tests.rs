use super::*;
use crate::types::AnchorRole;
use rune_vfs::{Mem, VfsTestExt};

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

#[test]
fn percent_decoded_target_resolves_to_the_percent_containing_file() {
    let vfs = mem_with(&["/root/archive/Canary tokens.md"]);
    let target = path_target("archive/Canary%20tokens.md");
    let dest = resolve(&vfs, &target, None, Some(Path::new("/root")), MD);
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
    let dest = resolve(&vfs, &target, None, Some(Path::new("/root")), MD);
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/root/100%.md"),
            anchor: None,
        }
    );
}

#[test]
fn a_target_containing_spaces_resolves() {
    let vfs = mem_with(&["/root/my notes/weekly review.md"]);
    let target = path_target("my notes/weekly review.md");
    let dest = resolve(&vfs, &target, None, Some(Path::new("/root")), MD);
    assert_eq!(
        dest,
        Destination::Location {
            path: PathBuf::from("/root/my notes/weekly review.md"),
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
    let dest = resolve(&vfs, &target, None, Some(Path::new("/root")), MD);
    assert_eq!(dest, Destination::Unresolved);
}

#[test]
fn url_target_resolves_without_touching_the_filesystem() {
    let vfs = Mem::new();
    let target = Target::Url("https://example.com".to_string());
    let dest = resolve(&vfs, &target, None, Some(Path::new("/root")), MD);
    assert_eq!(dest, Destination::Url("https://example.com".to_string()));
}

#[test]
fn resolve_returns_the_is_external_approved_value_not_the_raw_target() {
    let vfs = Mem::new();
    let target = Target::Url("  HTTPS://Example.com  ".to_string());
    let dest = resolve(&vfs, &target, None, Some(Path::new("/root")), MD);
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
            resolve(&vfs, &target, None, Some(Path::new("/root")), MD),
            Destination::Unresolved,
            "{hostile} must not resolve to a Url destination"
        );
    }
}
