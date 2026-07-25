//! WP3.S4: reveal-parity table tests, the `SyntaxSnapshot` round-trip
//! proptest (mirroring Go's `FuzzSyntaxMapRoundtrip`,
//! `pkg/editor/display/display_test.go`), and the single-transition-writer
//! grep gate (Ground rule 6).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use proptest::prelude::*;
use rune_core::buffer::Buffer;
use rune_core::coords::BufferPoint;
use rune_core::cursor::CursorSet;
use rune_md::element::RevealState;
use rune_md::element::doc::DocMachine;
use rune_md::emit::emit;

fn synced(content: &str, cursor_offset: usize, focused: bool) -> (Buffer, DocMachine) {
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_focus(focused);
    doc.sync_content(&buf);
    let offset = cursor_offset.min(buf.len());
    let cursors = CursorSet::new(offset);
    doc.sync_cursors(&buf, &cursors);
    (buf, doc)
}

fn joined_line(lines: &[rune_md::emit::SyntaxLine], line: usize) -> String {
    lines
        .get(line)
        .map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------
// (a) Reveal-parity table tests.
// ---------------------------------------------------------------------

#[test]
fn cursor_on_heading_line_reveals_marker() {
    let (buf, doc) = synced("## heading\nbody\n", 0, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks());
    assert_eq!(joined_line(&lines, 0), "## heading");
}

#[test]
fn cursor_off_heading_line_conceals_marker() {
    let (buf, doc) = synced("## heading\nbody\n", "## heading\n".len(), true);
    let (lines, _snap) = emit(buf.content(), doc.blocks());
    assert_eq!(joined_line(&lines, 0), "heading");
}

#[test]
fn cursor_inside_bold_reveals_with_nested_link_as_a_unit() {
    let content = "**[bo*ld*](url)** end\n";
    let cursor = content.find("ld").expect("fixture contains 'ld'");
    let (buf, doc) = synced(content, cursor, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks());
    assert_eq!(joined_line(&lines, 0), "**[bo*ld*](url)** end");
}

#[test]
fn cursor_outside_bold_conceals_delimiters_but_keeps_nested_text() {
    let content = "**[bo*ld*](url)** end\n";
    let (buf, doc) = synced(content, content.len(), true); // cursor on " end"
    let (lines, _snap) = emit(buf.content(), doc.blocks());
    assert_eq!(joined_line(&lines, 0), "bold end");
}

#[test]
fn cursor_inside_fence_reveals_whole_block_as_a_unit() {
    let content = "before\n```rust\nfn f() {}\n```\nafter\n";
    let cursor = content.find("fn f").expect("fixture contains code");
    let (buf, doc) = synced(content, cursor, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks());
    assert_eq!(joined_line(&lines, 1), "```rust");
    assert_eq!(joined_line(&lines, 2), "fn f() {}");
    assert_eq!(joined_line(&lines, 3), "```");
}

#[test]
fn cursor_outside_fence_conceals_fence_markers() {
    let content = "before\n```rust\nfn f() {}\n```\nafter\n";
    let (buf, doc) = synced(content, 0, true); // cursor on "before"
    let (lines, _snap) = emit(buf.content(), doc.blocks());
    assert_eq!(joined_line(&lines, 1), "");
    assert_eq!(joined_line(&lines, 2), "fn f() {}");
    assert_eq!(joined_line(&lines, 3), "");
}

#[test]
fn unfocused_renders_everything_concealed_even_on_cursor_line() {
    let content = "## heading\n**bold** text\n";
    // Cursor sits ON the heading line and inside the bold span — if focused,
    // both would reveal. Unfocused must force ForceRendered regardless
    // (Gotchas: "Unfocused -> ForceRendered").
    let (buf, doc) = synced(content, 0, false);
    let (lines, _snap) = emit(buf.content(), doc.blocks());
    assert_eq!(joined_line(&lines, 0), "heading");
    assert_eq!(joined_line(&lines, 1), "bold text");
    for block in doc.blocks() {
        assert_eq!(block.reveal_state(), RevealState::Rendered);
    }
}

#[test]
fn tasklist_marker_reveals_on_cursor_line() {
    let content = "- [x] task\nother\n";
    let (buf, doc) = synced(content, 0, true);
    let (lines, _snap) = emit(buf.content(), doc.blocks());
    assert_eq!(joined_line(&lines, 0), "- [x] task");
}

#[test]
fn tasklist_marker_conceals_off_cursor_line() {
    let content = "- [x] task\nother\n";
    let (buf, doc) = synced(content, "- [x] task\n".len(), true);
    let (lines, _snap) = emit(buf.content(), doc.blocks());
    assert_eq!(joined_line(&lines, 0), "task");
}

#[test]
fn blockquote_marker_reveals_per_line_independently() {
    let content = "> line one\n> line two\n";
    let (buf, doc) = synced(content, 0, true); // cursor on line 0 only
    let (lines, _snap) = emit(buf.content(), doc.blocks());
    assert_eq!(joined_line(&lines, 0), "> line one");
    assert_eq!(joined_line(&lines, 1), "line two");
}

// ---------------------------------------------------------------------
// (c) Single-transition-writer grep gate.
// ---------------------------------------------------------------------

/// Every RevealSm-shaped machine writes its state through exactly one
/// method (`RevealSm::transition` in `element/mod.rs`); the root machine
/// writes its own `DocState` through exactly one method
/// (`DocMachine::transition` in `element/doc.rs`). No other file under
/// `src/` may contain the literal write `self.state = next` — every other
/// machine reaches a state change only by calling `self.sm.transition(..)`.
#[test]
fn self_state_assignment_is_scoped_to_the_two_transition_writers() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let src_dir = manifest_dir.join("src");

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
    visit(&src_dir, &mut files);
    assert!(!files.is_empty(), "expected to find .rs files under src/");

    for file in &files {
        let contents = std::fs::read_to_string(file).unwrap_or_default();
        let count = contents.matches(needle).count();
        counts.push((file.clone(), count));
    }

    let element_mod = src_dir.join("element").join("mod.rs");
    let element_doc = src_dir.join("element").join("doc.rs");

    for (file, count) in &counts {
        if file == &element_mod || file == &element_doc {
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

// ---------------------------------------------------------------------
// (b) SyntaxSnapshot round-trip proptest — mirrors Go's
// FuzzSyntaxMapRoundtrip (pkg/editor/display/display_test.go).
// ---------------------------------------------------------------------

fn arb_markdown_fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("plain text".to_string()),
        Just("**bold**".to_string()),
        Just("*italic*".to_string()),
        Just("~~strike~~".to_string()),
        Just("`code`".to_string()),
        Just("[link](url)".to_string()),
        Just("[[wiki|label]]".to_string()),
        Just("# heading".to_string()),
        Just("## sub heading".to_string()),
        Just("> quoted line".to_string()),
        Just("- item".to_string()),
        Just("- [x] done task".to_string()),
        Just("- [ ] open task".to_string()),
        Just("1. ordered".to_string()),
        Just("---".to_string()),
        Just("```\nfenced\ncontent\n```".to_string()),
        Just("**[bo*ld*](url)**".to_string()),
        "[a-zA-Z0-9 ]{0,10}".prop_map(|s| s),
    ]
}

fn arb_content() -> impl Strategy<Value = String> {
    proptest::collection::vec(arb_markdown_fragment(), 0..8).prop_map(|frags| frags.join("\n"))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn syntax_map_roundtrip_is_identity_or_clamped_stable(
        content in arb_content(),
        raw_offset in any::<usize>(),
        focused in any::<bool>(),
    ) {
        let buf = Buffer::new(&content);
        let offset = if buf.is_empty() { 0 } else { raw_offset % (buf.len() + 1) };
        let mut doc = DocMachine::new();
        doc.set_focus(focused);
        doc.sync_content(&buf);
        let cursors = CursorSet::new(offset);
        doc.sync_cursors(&buf, &cursors);
        let (lines, snap) = emit(buf.content(), doc.blocks());

        for line in 0..buf.line_count() {
            let line_text = buf.line(line);
            for col in 0..=line_text.len() {
                let bp = BufferPoint { line, col };
                let sp = snap.buffer_to_syntax(bp);
                let bp2 = snap.syntax_to_buffer(sp);

                // Stability: the clamped position must be idempotent.
                let sp2 = snap.buffer_to_syntax(bp2);
                prop_assert_eq!(sp, sp2, "stability violated: bp={:?} sp={:?} bp2={:?} sp2={:?}", bp, sp, bp2, sp2);

                // If bp didn't round-trip, bp2 (its clamp target) must be
                // cursor-legal: it round-trips through sp2 exactly.
                if bp != bp2 {
                    let bp3 = snap.syntax_to_buffer(sp2);
                    prop_assert_eq!(bp2, bp3, "cursor-legal roundtrip failed: bp2={:?} sp2={:?} bp3={:?}", bp2, sp2, bp3);
                }
            }
        }

        // Every Rendered span's cell_map entries are -1 or valid char
        // boundaries within [buffer_start, buffer_end).
        for line in &lines {
            for span in &line.spans {
                if let Some(cm) = &span.cell_map {
                    prop_assert_eq!(span.state, RevealState::Rendered);
                    for &off in cm {
                        if off == -1 {
                            continue;
                        }
                        let off = off as usize;
                        prop_assert!(off >= span.buffer_start && off < span.buffer_end);
                        prop_assert!(buf.content().is_char_boundary(off));
                    }
                }
                // A Revealed span's text is always a direct, verbatim slice
                // of the buffer at its own recorded range.
                if span.state == RevealState::Revealed {
                    let expected = buf.content().get(span.buffer_start..span.buffer_end);
                    prop_assert_eq!(expected, Some(span.text.as_str()));
                }
            }
        }
    }
}
