//! Split off `conceal_roundtrip.rs` (WP11): the `SyntaxSnapshot`
//! round-trip proptest and its own markdown-fragment generator, built up
//! across every regression round this split's sibling files pin
//! individually.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use proptest::prelude::*;
use rune_core::buffer::Buffer;
use rune_core::coords::BufferPoint;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;
use rune_md::emit::emit;

/// Prefixes EVERY line of `inner` with `"> "` — a blockquote wrapping
/// (single application) or a nested blockquote (applied twice).
fn wrap_in_blockquote(inner: &str) -> String {
    inner
        .lines()
        .map(|l| format!("> {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wraps `inner` as a list item: `"- "` on the first line, `"  "` (the
/// marker's own width) on every continuation line — the list-item
/// counterpart of `wrap_in_blockquote`.
fn wrap_in_list_item(inner: &str) -> String {
    let mut out = String::new();
    for (i, l) in inner.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(if i == 0 { "- " } else { "  " });
        out.push_str(l);
    }
    out
}

/// Multi-line, block-shaped fragments worth nesting inside a container —
/// the fence-inside-blockquote/list-item shape was exactly this: a
/// multi-line block whose OWN lines each need the enclosing container's
/// prefix accounted for. The verification-round BLOCKER lived here.
fn arb_inner_block_fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("```rust\nfn f() {}\n```".to_string()),
        Just("```\ncode\n```".to_string()),
        Just("```rust\nline1\nline2\n```".to_string()),
        Just("```rust\nunterminated".to_string()),
        Just("# heading".to_string()),
        Just("---".to_string()),
        Just("**bold** text".to_string()),
        Just("plain text".to_string()),
        // Verification round 4's MAJOR shape, nested inside a container
        // too: an Emphasis/Strikethrough wrapping a multi-line wikilink
        // match, so the generator also reaches "the wrapper's own
        // child_gap_delims desync" INSIDE a blockquote/list item's own
        // line-prefix accounting, not just at top level.
        Just("*[[\n]]\n(*".to_string()),
        Just("~~[[\n]]\nb~~".to_string()),
        // HARNESS fix (verification round 5): the independent reviewer
        // found 2.81% of docs broken where this harness reported 0,
        // because these PRE-COMPOSED multi-line fragments — a setext
        // heading, a thematic break immediately followed by an empty
        // continuation line, a fence, a list item with an empty first
        // line — never appeared as ONE fragment the generator could
        // nest inside a container. Joining two SEPARATE top-level
        // fragments only reaches that adjacency by chance (that's how
        // rounds 4/5 originally found their bugs) — nesting requires the
        // adjacency to already exist INSIDE the one fragment being
        // wrapped.
        Just("x\n---".to_string()),
        Just("---\n>".to_string()),
        Just("- \n  > ".to_string()),
        Just("```\nc\n```".to_string()),
    ]
}

/// Wraps an inner block fragment in a container: single blockquote, nested
/// (doubly wrapped) blockquote, or a list item. This is what makes the
/// generator actually REACH "fence/heading/hr inside blockquote/list"
/// shapes — the previous generator only ever joined whole fragments at TOP
/// LEVEL (`arb_content`'s `frags.join("\n")`), so nothing it produced ever
/// put one block INSIDE another's container prefix, which is exactly why
/// it missed the fence-inside-container BLOCKER.
fn arb_container_wrapped_fragment() -> impl Strategy<Value = String> {
    arb_inner_block_fragment().prop_flat_map(|inner| {
        prop_oneof![
            Just(wrap_in_blockquote(&inner)),
            Just(wrap_in_blockquote(&wrap_in_blockquote(&inner))),
            Just(wrap_in_list_item(&inner)),
        ]
    })
}

/// An empty list item (no content on its OWN first line — just the
/// marker) followed by an indented continuation. Verification round 3's
/// BLOCKER shape: `arb_container_wrapped_fragment` above always wraps a
/// NON-empty inner fragment as the item's first line, so it can never
/// reach "the item's first line is empty" — this generator exists
/// specifically to cover that gap.
fn arb_empty_item_with_continuation() -> impl Strategy<Value = String> {
    let marker = prop_oneof![
        Just("-".to_string()),
        Just("- ".to_string()),
        Just("*".to_string()),
        Just("+".to_string()),
        Just("1.".to_string()),
        Just("\t-".to_string()),
    ];
    let continuation = prop_oneof![
        Just("  > q".to_string()),
        Just("> q".to_string()),
        Just("  x".to_string()),
        Just("x".to_string()),
        Just("  > nested\n  > more".to_string()),
    ];
    (marker, continuation).prop_map(|(m, c)| format!("{m}\n{c}"))
}

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
        // The shapes review round B2/B3/M1 caught: trailing/leading
        // whitespace, tabs, whitespace-only lines, indented code/lists,
        // CRLF, a closing ATX "##" sequence, backslash escapes, an empty
        // link, an unterminated fence, and a nested blockquote.
        Just("trailing   ".to_string()),
        Just("  leading indent".to_string()),
        Just("a\tb\tc".to_string()),
        Just("   ".to_string()),
        Just("    indented code".to_string()),
        Just("  - nested item".to_string()),
        Just("line one\r\nline two".to_string()),
        Just("## heading ##".to_string()),
        Just("\\*escaped\\*".to_string()),
        Just("[](url)".to_string()),
        Just("```rust\nunterminated".to_string()),
        Just("> > nested".to_string()),
        // Verification round 3's MAJOR + residual-producer shapes: a
        // wikilink label with leading whitespace and a CJK/emoji final
        // char (the comrak-child-sourcepos-unreliable case), a wikilink
        // whose own range spans multiple lines, and a tab-indented
        // blockquote continuation (comrak's own 0-3-SPACES-only
        // indentation rule for a repeated container marker).
        Just("[[ a]]".to_string()),
        Just("[[ 日]]".to_string()),
        Just("[[ 👍]]".to_string()),
        Just("[[\n]]".to_string()),
        Just(">]\n\t>".to_string()),
        // Verification round 4's MAJOR + control: an Emphasis/
        // Strikethrough wrapping a multi-line wikilink (the wrapper's
        // own child_gap_delims reads the corrupted last-child sourcepos
        // too), plus the unwrapped control that must stay clean.
        Just("*[[\n]]\n(*".to_string()),
        Just("*[[\n]]\n-*".to_string()),
        Just("~~[[\n]]\nb~~".to_string()),
        Just("[[\n]]\n(".to_string()),
        // HARNESS fix (verification round 5): pre-composed multi-line
        // fragments the independent reviewer's own generator carried but
        // this one didn't — CLASS A (lone-CR: CR, LF, or CRLF all end a
        // line per CommonMark, but only `\n` ends a BUFFER line) and
        // CLASS B (a thematic break immediately followed by an empty
        // blockquote continuation line) shapes, plus the reviewer's own
        // named fragments verbatim so the SAME adjacency this harness
        // missed is now a first-class alphabet member, not something
        // that only emerges by chance from two separate fragments.
        Just("a\r- n".to_string()),
        Just("a\r```".to_string()),
        Just("a\r# h".to_string()),
        Just("a\rb".to_string()),
        Just("a\r> q".to_string()),
        Just("a\r".to_string()),
        Just("a\r\n- n".to_string()),
        Just("> ---\n>".to_string()),
        Just("> ---\n> ".to_string()),
        Just("> ---\n> x".to_string()),
        Just(">===\n>".to_string()),
        Just("> a\n> ---\n>".to_string()),
        Just("> ---\n>>".to_string()),
        Just("> t\n> ---".to_string()),
        "[a-zA-Z0-9 ]{0,10}".prop_map(|s| s),
        // Verification-round BLOCKER shape: a block nested inside a
        // container (blockquote/nested-blockquote/list item).
        arb_container_wrapped_fragment(),
        // Verification round 3's BLOCKER shape: an empty list item marker
        // followed by an indented continuation.
        arb_empty_item_with_continuation(),
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
        doc.set_reveal_mode(focused);
        doc.sync_content(&buf);
        let cursors = CursorSet::new(offset);
        doc.sync_cursors(&buf, &cursors);
        let (lines, snap) = emit(buf.content(), doc.blocks(), 80);

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

        // Every Substituted span's cell_map entries are None or valid char
        // boundaries within its range.
        for line in &lines {
            for sp in &line.spans {
                let range = sp.range();
                if let rune_syntax::SyntaxSpan::Substituted { cell_map, .. } = sp {
                    for &off in cell_map {
                        let Some(off) = off else {
                            continue;
                        };
                        let off = off as usize;
                        prop_assert!(off >= range.start && off < range.end);
                        prop_assert!(buf.content().is_char_boundary(off));
                    }
                }
                // An Identical span's text is always a direct, verbatim
                // slice of the buffer at its own recorded range.
                if !sp.is_rendered() {
                    let expected = buf.content().get(range.clone());
                    prop_assert_eq!(expected, Some(sp.text(buf.content())));
                }
            }
        }

        // BLOCKER 1's invariant, strengthened from clamp-stability alone
        // (which BL2 passed while badly wrong): per line, every visible
        // span's buffer-byte length plus every hidden range's byte length
        // must equal the line's exact byte length — no byte is ever
        // silently dropped (trailing/leading whitespace, tabs, a bare `\r`,
        // an ATX heading's closing "#"-run, anything a comrak sourcepos
        // doesn't happen to span).
        for line in 0..buf.line_count() {
            let expected_len = buf.line(line).len();
            let visible: usize = lines
                .get(line)
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| {
                            let r = s.range();
                            r.end.saturating_sub(r.start)
                        })
                        .sum()
                })
                .unwrap_or(0);
            let hidden = snap.hidden_byte_count(line);
            prop_assert_eq!(visible + hidden, expected_len, "line {} coverage gap: visible {} + hidden {} != length {}", line, visible, hidden, expected_len);

            // When a line has NO hidden ranges at all (nothing on it is
            // concealed — note this is a different question from "every
            // span is Identical": a Text run nested inside a CONCEALED
            // emphasis is still tagged Identical itself, since the variant
            // marks "this run is a verbatim buffer copy", not "nothing
            // wrapping it is hidden" — only `hidden_byte_count` answers the
            // line-wide question), the concatenated span text
            // must equal the exact buffer line bytes.
            if let Some(l) = lines.get(line)
                && !l.spans.is_empty()
                && hidden == 0
            {
                let joined: String = l.spans.iter().map(|s| s.text(buf.content())).collect();
                prop_assert_eq!(joined, buf.line(line));
            }
        }
    }
}
