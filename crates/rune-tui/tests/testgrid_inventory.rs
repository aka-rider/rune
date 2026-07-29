//! WP13.S5 (`rune-tui C 8`): `testgrid.rs`'s own module doc claims it is
//! "the one place left in the crate that constructs a `TestBackend`" — an
//! invariant that had already silently rotted (`opentabs.rs`'s and
//! `title.rs`'s own test modules each grew a second, hand-rolled copy of
//! the same draw-into-a-`TestBackend` boilerplate). This test makes the
//! claim self-checking instead of a comment nobody re-verifies: it walks
//! every `.rs` file under `src/` and asserts `TestBackend::new` appears
//! exactly once crate-wide, in `testgrid.rs` itself.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn test_backend_is_constructed_in_exactly_one_source_file() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = manifest_dir.join("src");

    let mut files = Vec::new();
    visit(&src, &mut files);
    assert!(!files.is_empty(), "expected to find .rs files under src/");

    // The call form, not the bare type path — so a doc comment merely
    // NAMING `TestBackend::new` (as this module's own docs, and this
    // test's, both do) can't inflate the count.
    let needle = "TestBackend::new(";
    let testgrid_rs = src.join("testgrid.rs");
    let mut construction_sites: Vec<(PathBuf, usize)> = Vec::new();

    for file in &files {
        let contents = std::fs::read_to_string(file).unwrap_or_default();
        let count = contents.matches(needle).count();
        if count > 0 {
            construction_sites.push((file.clone(), count));
        }
    }

    assert_eq!(
        construction_sites,
        vec![(testgrid_rs, 1)],
        "`TestBackend::new` must appear exactly once, in testgrid.rs — every \
         other test module must draw through `testgrid::draw`/`draw_with`, \
         found: {construction_sites:?}"
    );
}
