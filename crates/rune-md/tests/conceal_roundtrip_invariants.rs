//! Split off `conceal_roundtrip.rs` (WP11, §1.6): the single-transition-
//! writer grep gate (Ground rule 6). No shared fixtures needed — this test
//! walks source files directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

/// Every RevealSm-shaped machine writes its state through exactly one
/// method (`RevealSm::transition`, in `rune-syntax`'s own `element.rs`
/// since WP3 moved it out of `rune-md`'s own `element/mod.rs`); the root
/// machine writes its own `DocState` through exactly one method
/// (`DocMachine::transition` in `element/doc.rs`, which stays in
/// `rune-md`). No other file under either crate's `src/` may contain the
/// literal write `self.state = next` — every other machine reaches a state
/// change only by calling `self.sm.transition(..)`.
#[test]
fn self_state_assignment_is_scoped_to_the_two_transition_writers() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let rune_md_src = manifest_dir.join("src");
    let rune_syntax_src = manifest_dir.join("..").join("rune-syntax").join("src");

    let needle = "self.state = next";
    let mut counts: Vec<(std::path::PathBuf, usize)> = Vec::new();

    fn visit(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
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

    let mut files = Vec::new();
    visit(&rune_md_src, &mut files);
    visit(&rune_syntax_src, &mut files);
    assert!(!files.is_empty(), "expected to find .rs files under src/");

    for file in &files {
        let contents = std::fs::read_to_string(file).unwrap_or_default();
        let count = contents.matches(needle).count();
        counts.push((file.clone(), count));
    }

    let element_doc = rune_md_src.join("element").join("doc.rs");
    let syntax_element = rune_syntax_src.join("element.rs");

    for (file, count) in &counts {
        if file == &element_doc || file == &syntax_element {
            assert_eq!(
                *count, 1,
                "{file:?} must contain exactly one `{needle}` write (its own transition writer), found {count}"
            );
        } else {
            assert_eq!(
                *count, 0,
                "{file:?} must not write `{needle}` directly — every other machine calls \
                 `self.sm.transition(..)` instead, found {count}"
            );
        }
    }
}
