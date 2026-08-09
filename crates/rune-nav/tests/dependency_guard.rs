//! [rune-nav 8]: the module doc's "must never depend on rune-md or
//! rune-tui" binding was, before this test, a written promise with zero
//! mechanical enforcement — nothing in CI would catch a dependency edge
//! reintroducing either crate. This test walks the real `cargo metadata`
//! resolve graph for rune-nav's own package id and fails the instant
//! either forbidden crate appears anywhere in its transitive dependency
//! closure, not just as a direct `Cargo.toml` entry — a path THROUGH some
//! other crate is caught too.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::{HashSet, VecDeque};
use std::process::Command;

use serde_json::Value;

fn workspace_metadata() -> Value {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let workspace_manifest = format!("{manifest_dir}/../../Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version=1", "--manifest-path"])
        .arg(&workspace_manifest)
        .output()
        .expect("cargo metadata must run");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("cargo metadata prints valid JSON")
}

fn package_name(metadata: &Value, id: &str) -> String {
    metadata["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .find(|p| p["id"] == id)
        .and_then(|p| p["name"].as_str())
        .unwrap_or(id)
        .to_string()
}

fn find_package_id(metadata: &Value, crate_name: &str) -> String {
    metadata["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .find(|p| p["name"] == crate_name)
        .unwrap_or_else(|| panic!("{crate_name} present in `cargo metadata` output"))["id"]
        .as_str()
        .expect("package id is a string")
        .to_string()
}

fn node_deps<'a>(metadata: &'a Value, id: &str) -> Vec<&'a str> {
    metadata["resolve"]["nodes"]
        .as_array()
        .expect("resolve.nodes array")
        .iter()
        .find(|n| n["id"] == id)
        .map(|n| {
            n["dependencies"]
                .as_array()
                .expect("node dependencies array")
                .iter()
                .map(|d| d.as_str().expect("dependency id is a string"))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn rune_nav_never_transitively_depends_on_rune_md_or_rune_tui() {
    let metadata = workspace_metadata();
    let root = find_package_id(&metadata, "rune-nav");

    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(root.clone());
    visited.insert(root.clone());

    while let Some(id) = queue.pop_front() {
        for dep_id in node_deps(&metadata, &id) {
            if visited.insert(dep_id.to_string()) {
                queue.push_back(dep_id.to_string());
            }
        }
    }
    visited.remove(&root);

    let forbidden: Vec<String> = visited
        .iter()
        .map(|id| package_name(&metadata, id))
        .filter(|name| name == "rune-md" || name == "rune-tui")
        .collect();

    assert!(
        forbidden.is_empty(),
        "rune-nav's resolved dependency graph must never include rune-md or \
         rune-tui; found: {forbidden:?}"
    );
}
