//! Split off `conceal_roundtrip.rs` (WP11): the single-transition-
//! writer grep gate (Ground rule 6). No shared fixtures needed — this test
//! walks source files directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

/// Every RevealSm-shaped machine writes its state through exactly one
/// method (`RevealSm::transition`, in `rune-syntax`'s own `element.rs`
/// since WP3 moved it out of `rune-md`'s own `element/mod.rs`); the root
/// machine writes its own `RevealMode` through exactly one method
/// (`DocMachine::transition` in `element/doc.rs`, which stays in
/// `rune-md`). No other file under either crate's `src/` may contain either
/// literal write — every other machine reaches a state change only by
/// calling `self.sm.transition(..)`.
#[test]
fn self_state_assignment_is_scoped_to_the_two_transition_writers() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let rune_md_src = manifest_dir.join("src");
    let rune_syntax_src = manifest_dir.join("..").join("rune-syntax").join("src");

    let element_doc = rune_md_src.join("element").join("doc.rs");
    let syntax_element = rune_syntax_src.join("element.rs");

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

    // Each transition writer has its own field name: `RevealSm::transition`
    // writes `state` (rune-syntax's `element.rs`); `DocMachine::transition`
    // writes `reveal_mode` (rune-md's `element/doc.rs`). Every other file
    // must contain neither literal write.
    for file in &files {
        let contents = std::fs::read_to_string(file).unwrap_or_default();
        let state_count = contents.matches("self.state = next").count();
        let reveal_mode_count = contents.matches("self.reveal_mode = next").count();

        if file == &syntax_element {
            assert_eq!(
                state_count, 1,
                "{file:?} must contain exactly one `self.state = next` write (its own transition writer), found {state_count}"
            );
        } else {
            assert_eq!(
                state_count, 0,
                "{file:?} must not write `self.state = next` directly — every other machine calls \
                 `self.sm.transition(..)` instead, found {state_count}"
            );
        }

        if file == &element_doc {
            assert_eq!(
                reveal_mode_count, 1,
                "{file:?} must contain exactly one `self.reveal_mode = next` write (its own transition writer), found {reveal_mode_count}"
            );
        } else {
            assert_eq!(
                reveal_mode_count, 0,
                "{file:?} must not write `self.reveal_mode = next` directly, found {reveal_mode_count}"
            );
        }
    }
}
