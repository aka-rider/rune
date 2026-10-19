#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

const CONSTRUCTOR: &str = "Violation::new(";
const PANIC_ID: &str = "NO-PANIC";

fn rust_sources(dir: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(fs::read_to_string(&path).unwrap());
        }
    }
}

fn emitted_ids() -> BTreeSet<String> {
    let mut sources = Vec::new();
    rust_sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut sources,
    );
    let mut ids = BTreeSet::from([PANIC_ID.to_string()]);
    for source in &sources {
        for (at, _) in source.match_indices(CONSTRUCTOR) {
            let rest = source[at + CONSTRUCTOR.len()..].trim_start();
            if let Some(literal) = rest.strip_prefix('"')
                && let Some(end) = literal.find('"')
            {
                ids.insert(literal[..end].to_string());
            }
        }
    }
    ids
}

fn roster() -> (usize, BTreeSet<String>) {
    let lib = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs")).unwrap();
    let declared = lib
        .lines()
        .find_map(|line| line.strip_prefix("//! # Invariant roster ("))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("the roster header states its total")
        .parse()
        .expect("the roster total is a number");
    let ids = lib
        .lines()
        .filter_map(|line| line.strip_prefix("//! - `"))
        .filter_map(|rest| rest.split('`').next())
        .map(str::to_string)
        .collect();
    (declared, ids)
}

#[test]
fn every_emitted_invariant_id_is_in_the_roster_and_nothing_else_is() {
    let (_, listed) = roster();
    let emitted = emitted_ids();
    let unlisted: Vec<_> = emitted.difference(&listed).collect();
    let phantom: Vec<_> = listed.difference(&emitted).collect();
    assert!(
        unlisted.is_empty() && phantom.is_empty(),
        "roster drift: emitted but not listed {unlisted:?}; listed but never emitted {phantom:?}"
    );
}

#[test]
fn the_roster_header_total_matches_the_number_of_entries() {
    let (declared, listed) = roster();
    assert_eq!(declared, listed.len());
}
