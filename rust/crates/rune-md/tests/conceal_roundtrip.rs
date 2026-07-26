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
// (a2) Per-byte coverage regression cases (review BLOCKER 1/2/3, MAJOR 4):
// every byte of every line is either visible or hidden — never dropped.
// ---------------------------------------------------------------------

/// The invariant BLOCKER 1 violated: per line, the sum of every span's
/// buffer-byte length plus every hidden range's byte length must equal the
/// line's exact byte length. A per-LINE `touched` bool couldn't distinguish
/// a partially-covered line from a fully-covered one; this checks BYTES.
fn assert_full_line_coverage(
    buf: &Buffer,
    lines: &[rune_md::emit::SyntaxLine],
    snap: &rune_md::emit::SyntaxSnapshot,
) {
    for line in 0..buf.line_count() {
        let expected_len = buf.line(line).len();
        let visible: usize = lines
            .get(line)
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.buffer_end.saturating_sub(s.buffer_start))
                    .sum()
            })
            .unwrap_or(0);
        let hidden = snap.hidden_byte_count(line);
        assert_eq!(
            visible + hidden,
            expected_len,
            "line {line} ({:?}): visible {visible} + hidden {hidden} != line length {expected_len} — a byte was silently dropped",
            buf.line(line)
        );
    }
}

#[test]
fn trailing_whitespace_is_visible_not_dropped() {
    let (buf, doc) = synced("hello   \nnext\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0), "hello   ");
}

#[test]
fn leading_indent_is_visible_not_dropped() {
    let (buf, doc) = synced("  leading spaces\nnext\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0), "  leading spaces");
}

#[test]
fn embedded_tab_is_visible_not_dropped() {
    let (buf, doc) = synced("a\tb\nnext\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0), "a\tb");
}

#[test]
fn whitespace_only_line_is_visible_not_dropped() {
    let (buf, doc) = synced("para\n   \nnext\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
}

#[test]
fn indented_code_block_is_visible_not_dropped() {
    let (buf, doc) = synced("para\n\n    indented code\n\nafter\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 2), "    indented code");
}

#[test]
fn indented_list_marker_is_visible_not_dropped() {
    let (buf, doc) = synced("  - nested item\nnext\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
}

#[test]
fn crlf_carriage_return_is_visible_not_dropped() {
    let (buf, doc) = synced("line one\r\nline two\r\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
    // The bare \r before \n is user content (§1.4.5) — it must show up in
    // the concealed/revealed text exactly as written.
    assert!(
        joined_line(&lines, 0).ends_with('\r'),
        "line 0 = {:?}",
        joined_line(&lines, 0)
    );
}

#[test]
fn atx_heading_closing_sequence_is_visible_not_dropped() {
    // CommonMark strips an optional trailing "#"-run from an ATX heading's
    // CONTENT, but those trailing bytes are still part of the raw line —
    // they must show up as visible text when concealed, not vanish.
    let (buf, doc) = synced("## heading ##\nnext\n", "## heading ##\n".len(), true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
}

#[test]
fn backslash_escape_is_visible_not_dropped() {
    let (buf, doc) = synced("\\*not bold\\*\nnext\n", 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
}

#[test]
fn empty_link_hides_exactly_once() {
    // BLOCKER 2: an empty-text link's open/close delimiter fallbacks used to
    // both default to the whole token range, double-hiding it and breaking
    // buffer_to_syntax monotonicity.
    let content = "see [](http://x) here\n";
    let (buf, doc) = synced(content, 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0), "see  here");

    // buffer_to_syntax must be monotonic non-decreasing across the line.
    let mut prev = None;
    for col in 0..=content.trim_end_matches('\n').len() {
        let sp = snap.buffer_to_syntax(BufferPoint { line: 0, col });
        if let Some(p) = prev {
            assert!(
                sp.col >= p,
                "buffer_to_syntax not monotonic at col {col}: prev={p} now={}",
                sp.col
            );
        }
        prev = Some(sp.col);
    }
}

#[test]
fn unterminated_fence_keeps_every_line_visible_content() {
    // BLOCKER 3: `last_line > first_line` alone was wrongly treated as
    // "closing fence exists" — an in-progress (unterminated) fence lost its
    // last content line to a phantom fence_close.
    let content = "```rust\nfn f() {}\nlet x = 1;\n";
    let (buf, doc) = synced(content, 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
    // Cursor away (unfocused-equivalent conceal): every content line must
    // still show its text — nothing after the opening fence is a phantom
    // closing marker.
    assert_eq!(joined_line(&lines, 1), "fn f() {}");
    assert_eq!(joined_line(&lines, 2), "let x = 1;");
}

#[test]
fn nested_blockquote_markers_are_at_their_true_depth_offset() {
    // MAJOR 4: both depths used to report marker range [0,2), double-hiding
    // the same 2 bytes and leaving the inner "> " at [2,4) unmodeled.
    let content = "> > nested quote\n";
    let (buf, doc) = synced(content, content.len(), true); // cursor away: both conceal
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0), "nested quote");
}

// ---------------------------------------------------------------------
// (a3) Fence-inside-container regression cases (verification-round
// BLOCKER): fence_open/content/fence_close used to be derived from
// PHYSICAL line extents (line_start_at/line_end_at), ignoring the
// enclosing container's own prefix already claimed on that line — the
// fence's ranges swallowed bytes the container's marker had already hidden
// (or, for the Revealed dump, already shown), so every position past the
// collision was off by the doubly-counted delta. Checked in BOTH focus
// states, per line: full byte coverage, `buffer_to_syntax` monotonic
// non-decreasing across the line, and round-trip stability.
// ---------------------------------------------------------------------

fn assert_container_fence_invariants(content: &str) {
    for &focused in &[true, false] {
        let (buf, doc) = synced(content, 0, focused);
        let (lines, snap) = emit(buf.content(), doc.blocks());
        assert_full_line_coverage(&buf, &lines, &snap);

        for line in 0..buf.line_count() {
            let line_len = buf.line(line).len();
            let mut prev_syntax_col = None;
            for col in 0..=line_len {
                let bp = BufferPoint { line, col };
                let sp = snap.buffer_to_syntax(bp);
                if let Some(prev) = prev_syntax_col {
                    assert!(
                        sp.col >= prev,
                        "buffer_to_syntax not monotonic (focused={focused}) at line {line} col {col}: prev={prev} now={}",
                        sp.col
                    );
                }
                prev_syntax_col = Some(sp.col);

                // Round-trip stability: buffer_to_syntax(syntax_to_buffer(sp))
                // == sp for every syntax point reachable from this line.
                let bp2 = snap.syntax_to_buffer(sp);
                let sp2 = snap.buffer_to_syntax(bp2);
                assert_eq!(
                    sp, sp2,
                    "round-trip stability failed (focused={focused}) at line {line} col {col}: bp={bp:?} sp={sp:?} bp2={bp2:?} sp2={sp2:?}"
                );
            }
        }
    }
}

#[test]
fn fence_inside_blockquote_container_prefix_not_double_hidden() {
    // The reviewer's exact repro: unfocused line 0 used to report
    // visible=0 + hidden=11 on a 9-byte line ("> ```rust"), and
    // syntax_to_buffer(0) returned col 11 — out of the line entirely.
    assert_container_fence_invariants("> ```rust\n> fn main() {}\n> ```\n");
}

#[test]
fn fence_inside_bare_blockquote_container_prefix_not_double_hidden() {
    assert_container_fence_invariants("> ```\n> code\n> ```\n");
}

#[test]
fn fence_inside_nested_blockquote_container_prefix_not_double_hidden() {
    assert_container_fence_invariants("> > ```\n> > c\n> > ```\n");
}

#[test]
fn fence_inside_list_item_container_prefix_not_double_hidden() {
    // fence_open on line 0 used to span the whole physical line
    // ("- ```rust"), colliding with the list item's own marker [0,2).
    assert_container_fence_invariants("- ```rust\n  code\n  ```\n");
}

#[test]
fn fence_with_multiple_content_lines_inside_blockquote() {
    // Every content line (not just the first/last) carries the
    // container's repeating "> " prefix — this is the shape a single
    // contiguous `content` range could never handle correctly.
    assert_container_fence_invariants("> ```rust\n> line1\n> line2\n> ```\n");
}

// ---------------------------------------------------------------------
// (a4) Empty-list-item-marker regression cases (verification round 3
// BLOCKER): an empty item's marker ran from the item's own start to its
// FIRST CHILD's start — which, for a lazily-indented continuation (e.g. a
// nested blockquote under "- \n  > q"), sits on the NEXT physical line.
// The marker swallowed that line's leading indent, bytes the
// continuation's own scan (`blockquote_markers`) claims independently:
// content invented on the visible side (both spans show the same 2
// bytes) — §1.4.5's mirror image of dropping a byte.
// ---------------------------------------------------------------------

#[allow(clippy::needless_range_loop)] // `line` also indexes buf.line()/snap.hidden_byte_count(), not just `lines`
fn assert_no_duplicate_content(content: &str) {
    for &focused in &[true, false] {
        let (buf, doc) = synced(content, 0, focused);
        let (lines, snap) = emit(buf.content(), doc.blocks());
        assert_full_line_coverage(&buf, &lines, &snap);

        for line in 0..buf.line_count() {
            let line_len = buf.line(line).len();
            let l = &lines[line];

            // No two spans on this line may claim overlapping buffer
            // bytes — the literal "content duplicated" shape.
            for i in 0..l.spans.len() {
                for j in (i + 1)..l.spans.len() {
                    let a = &l.spans[i];
                    let b = &l.spans[j];
                    assert!(
                        a.buffer_end <= b.buffer_start || b.buffer_end <= a.buffer_start,
                        "line {line} (focused={focused}): spans {i} {a:?} and {j} {b:?} claim overlapping buffer bytes"
                    );
                }
            }

            // When nothing on this line is hidden, the emitted text must
            // equal the exact buffer bytes — not longer (duplicated
            // content) or shorter (dropped content).
            if snap.hidden_byte_count(line) == 0 {
                let joined: String = l.spans.iter().map(|s| s.text.as_str()).collect();
                assert_eq!(
                    joined,
                    buf.line(line),
                    "line {line} (focused={focused}): rendered text != exact buffer bytes"
                );
            }

            let mut prev_syntax_col = None;
            for col in 0..=line_len {
                let bp = BufferPoint { line, col };
                let sp = snap.buffer_to_syntax(bp);
                if let Some(prev) = prev_syntax_col {
                    assert!(
                        sp.col >= prev,
                        "buffer_to_syntax not monotonic (focused={focused}) at line {line} col {col}: prev={prev} now={}",
                        sp.col
                    );
                }
                prev_syntax_col = Some(sp.col);

                let bp2 = snap.syntax_to_buffer(sp);
                let sp2 = snap.buffer_to_syntax(bp2);
                assert_eq!(
                    sp, sp2,
                    "round-trip stability failed (focused={focused}) at line {line} col {col}: bp={bp:?} sp={sp:?} bp2={bp2:?} sp2={sp2:?}"
                );

                // The reported symptom: syntax_to_buffer mapping past the
                // buffer line's own end.
                assert!(
                    bp2.col <= line_len,
                    "syntax_to_buffer mapped past end-of-line (focused={focused}) at line {line} col {col}: bp2={bp2:?} line_len={line_len}"
                );
            }
        }
    }
}

#[test]
fn empty_list_item_marker_does_not_duplicate_continuation_indent() {
    // The reviewer's exact repro: buffer line 1 is "  > q" (5 bytes) but
    // the marker used to swallow the 2-space indent a second time,
    // emitting "    > q" (7 bytes).
    assert_no_duplicate_content("- \n  > q");
}

#[test]
fn empty_item_variants_times_continuation_matrix() {
    let empty_markers = ["-", "- ", "*", "+", "1.", "\t-"];
    let continuations = ["  > q", "> q", "  x", "x"];
    for m in empty_markers {
        for c in continuations {
            assert_no_duplicate_content(&format!("{m}\n{c}"));
        }
    }
}

#[test]
fn empty_item_continuation_controls_stay_clean() {
    // Known-good controls that must remain clean.
    assert_no_duplicate_content("- a\n  > q");
    assert_no_duplicate_content("-\n>");
    assert_no_duplicate_content("-\n  x");
}

// ---------------------------------------------------------------------
// (a5) Wikilink label-range regression cases (verification round 3
// MAJOR): the label range used to be read off comrak's own child-node
// sourcepos, which is unreliable for a WikiLink target with leading
// whitespace that gets trimmed — off by one for ASCII ("[[ a]]" showed
// " " instead of " a"), and char-splitting/out-of-range for a multibyte
// final char ("[[ 日]]", "[[ 👍]]"), which used to hit the emit-site
// `else { continue }` and silently drop the whole span (bytes
// unaccounted for — the round-1 byte-loss class at a new site). Checked
// revealed (cursor on the wikilink's own line), concealed (cursor on an
// unrelated line), and unfocused (always concealed) — plus full
// per-line byte coverage in every state.
// ---------------------------------------------------------------------

fn assert_wikilink_label(content: &str, concealed_label: &str) {
    // Revealed: cursor ON the wikilink's own line shows the raw markup
    // verbatim — nothing about revealing depends on the label arithmetic.
    let (buf, doc) = synced(content, 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0), content.trim_end_matches('\n'));

    // Concealed: cursor on an unrelated line shows just the label.
    let wrapped = format!("x\n{content}");
    let (buf, doc) = synced(&wrapped, 0, true);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 1), concealed_label);

    // Unfocused: always concealed regardless of cursor position.
    let (buf, doc) = synced(content, 0, false);
    let (lines, snap) = emit(buf.content(), doc.blocks());
    assert_full_line_coverage(&buf, &lines, &snap);
    assert_eq!(joined_line(&lines, 0), concealed_label);
}

#[test]
fn wikilink_label_with_leading_space_ascii_is_byte_exact() {
    assert_wikilink_label("[[ a]]\n", " a");
}

#[test]
fn wikilink_label_with_leading_space_cjk_is_byte_exact() {
    assert_wikilink_label("[[ 日]]\n", " 日");
}

#[test]
fn wikilink_label_with_leading_space_emoji_is_byte_exact() {
    assert_wikilink_label("[[ 👍]]\n", " 👍");
}

// ---------------------------------------------------------------------
// (a6) Residual-producer regression cases (verification round 3:
// advisory promoted to work): two inputs still tripped the
// strict-invariants assert, saved only by the emit-site chokepoint —
// both producers now line-clamp/disjoint their claims like every other
// producer in this crate, so these are green even under strict mode.
// ---------------------------------------------------------------------

#[test]
fn multiline_wikilink_does_not_claim_across_lines() {
    // A wikilink whose own sourcepos spans more than one physical line
    // degrades to plain text — no single-line home for open/close
    // delimiter claims exists, so it must never reach WikiLinkM
    // construction in the first place.
    for &focused in &[true, false] {
        let (buf, doc) = synced("[[\n]]\n", 0, focused);
        let (lines, snap) = emit(buf.content(), doc.blocks());
        assert_full_line_coverage(&buf, &lines, &snap);
    }
}

#[test]
fn tab_indented_blockquote_continuation_does_not_double_claim() {
    // comrak treats a TAB-indented continuation line as lazy-continuation
    // PARAGRAPH TEXT (CommonMark: a repeated container marker may only be
    // preceded by 0-3 SPACES, never a tab, which represents 4 columns) —
    // `blockquote_markers` used to recognize it as a repeated ">" marker
    // anyway (`str::trim_start` strips tabs too), double-claiming the
    // same byte the paragraph's own Text node also claims: a
    // producer-bug duplicate-claim panic under strict invariants.
    assert_no_duplicate_content(">]\n\t>");
}

// ---------------------------------------------------------------------
// (a7) Emphasis/strikethrough wrapping a multi-line wikilink
// (verification round 4 MAJOR): comrak's line-counter desync (round 3's
// "[[\n]]" root cause) doesn't stop at the wikilink's own siblings — a
// PARENT wrapping it is exposed too, because its own `child_gap_delims`
// reads the LAST child's (possibly corrupted) sourcepos to place the
// close delimiter. `"*[[\n]]\n(*"`: the closing "*" got recorded hidden
// on the wikilink's own line while the emitter placed it, unhidden, on
// the real closing line — a coverage/duplicate-claim bug at a new site,
// same root cause as round 3's residual producers.
// ---------------------------------------------------------------------

#[test]
fn emphasis_wrapped_multiline_wikilink_does_not_double_claim() {
    assert_no_duplicate_content("*[[\n]]\n(*");
    assert_no_duplicate_content("*[[\n]]\n-*");
}

#[test]
fn strikethrough_wrapped_multiline_wikilink_does_not_double_claim() {
    assert_no_duplicate_content("~~[[\n]]\nb~~");
}

#[test]
fn multiline_wikilink_without_wrapper_stays_clean() {
    // The reviewer's clean control: without the Emphasis/Strikethrough
    // wrapper, a bare multi-line wikilink followed by more content was
    // already correctly handled by round 3's fix — pinned here so a
    // future change can't silently regress the unwrapped case while
    // fixing the wrapped one.
    assert_no_duplicate_content("[[\n]]\n(");
}

// ---------------------------------------------------------------------
// (a8) Setext heading nested in a container (verification round 4
// MAJOR, second site — same comrak-desync family of "a multi-line
// construct's own raw range fed whole into the generic per-line
// splitter"): `HeadingM::range` spans BOTH the text line and the
// "==="/"---" underline for a setext heading. Pushing that whole span
// through the Revealed emit path's per-physical-line splitter re-claims
// a REPEATING container prefix (a blockquote's "> ") on the underline's
// own continuation line, on top of what the blockquote's own marker
// scan already (and correctly) claims there — fixed the same way
// `CodeFenceM` already was: per-line, `hint`-aware `content_lines` built
// at parse time, never `range` used whole at emit time. Unlike round 4's
// wikilink MAJOR, this one is comrak-desync-FREE — comrak's own
// sourcepos for a setext heading is entirely reliable; the bug was
// purely in how this crate turned a reliable multi-line range into
// per-line pieces.
// ---------------------------------------------------------------------

#[test]
fn setext_heading_nested_in_double_blockquote_does_not_double_claim() {
    assert_no_duplicate_content("> > nested\n> > ---");
}

#[test]
fn setext_heading_nested_in_blockquote_does_not_double_claim() {
    assert_no_duplicate_content("> nested\n> ---");
}

#[test]
fn setext_heading_nested_in_list_item_does_not_double_claim() {
    assert_no_duplicate_content("- nested\n  ---");
}

#[test]
fn setext_heading_with_trailing_content_lines_stays_clean() {
    // Content AFTER the underline (a later continuation line, nested or
    // not) must stay unaffected — the fix only changes how the heading's
    // OWN two lines are split, never anything past them.
    assert_no_duplicate_content("> nested\n> ---\n> more text");
    assert_no_duplicate_content("nested\n---\nafter");
}

// ---------------------------------------------------------------------
// (a9) CLASS A (verification round 5): a lone `\r` line terminator.
// comrak follows CommonMark: CR, LF, or CRLF all end a line. This
// crate's BUFFER line model is `\n`-only (Go parity, §1.5) — correctly
// so, a bare `\r` is ordinary mid-line content, never a buffer line
// break. But `sourcepos_to_range` used to convert comrak's own
// (CR/LF/CRLF-aware) sourcepos through that SAME `\n`-only index, so the
// moment content contained a bare `\r`, comrak's line N stopped matching
// this crate's line N and every downstream byte offset landed on the
// wrong physical position. Fixed with a SECOND, comrak-compatible line
// index (`LineIndex::comrak`) used ONLY for sourcepos conversion — the
// bytes themselves are never touched (§1.4.5).
// ---------------------------------------------------------------------

#[test]
fn lone_cr_before_list_marker_does_not_desync() {
    assert_no_duplicate_content("a\r- n");
}

#[test]
fn lone_cr_before_fence_does_not_desync() {
    assert_no_duplicate_content("a\r```");
}

#[test]
fn lone_cr_before_heading_does_not_desync() {
    assert_no_duplicate_content("a\r# h");
}

#[test]
fn lone_cr_controls_stay_clean() {
    // The reviewer's clean controls: plain text and a blockquote after a
    // lone CR, plus a CR with nothing following it at all.
    assert_no_duplicate_content("a\rb");
    assert_no_duplicate_content("a\r> q");
    assert_no_duplicate_content("a\r");
}

#[test]
fn crlf_before_list_marker_stays_clean() {
    // The CRLF control: CRLF is ONE terminator, not two — must NOT be
    // treated as a lone CR immediately followed by a lone LF (which
    // would double-count a line break that only happened once).
    assert_no_duplicate_content("a\r\n- n");
}

#[test]
fn lone_cr_inside_frontmatter_does_not_desync_the_rest_of_the_document() {
    // A THIRD comrak-extension desync, found by this round's own widened
    // generator (not part of the reviewer's original CLASS A report): a
    // lone `\r` inside a frontmatter block's body throws off comrak's
    // frontmatter-closing search (which appears to scan by `\n`-only
    // splitting internally) relative to the CR/LF/CRLF-aware line
    // counter the REST of comrak's block parser keeps counting from
    // afterward — corrupting every LATER block's sourcepos too, not just
    // the frontmatter block's own (the wikilink-extension desync's
    // document-wide sibling). `parse()`'s `frontmatter_extension_is_safe`
    // pre-check detects this and re-parses with the extension disabled.
    assert_no_duplicate_content("---\na\r```\n---\n> nested");
}

// ---------------------------------------------------------------------
// (a10) CLASS B (verification round 5): an empty blockquote
// continuation line immediately after a thematic break ("> ---\n>").
// CommonMark's own grammar makes a thematic break exactly ONE line, but
// comrak's reported sourcepos for one immediately followed by an EMPTY
// blockquote continuation line extended THROUGH that next line's own
// "> " marker — a hidden-side double-claim: the marker byte was BOTH
// counted hidden by the blockquote's own marker scan AND swept into the
// (un-clamped) thematic break's own range. Fixed by clamping `HrM::range`
// to its own single line, the same shape as `ListItemM`'s marker clamp.
// ---------------------------------------------------------------------

#[test]
fn thematic_break_before_empty_quote_continuation_does_not_double_claim() {
    assert_no_duplicate_content("> ---\n>");
    assert_no_duplicate_content("> ---\n> ");
    assert_no_duplicate_content("> ---\n>\n");
    assert_no_duplicate_content("> ---\n> \n");
}

#[test]
fn thematic_break_empty_continuation_controls_stay_clean() {
    // The reviewer's clean controls: a NON-empty continuation line, "==="
    // (not a valid thematic break marker, so a different node kind
    // entirely), a setext heading (a REAL multi-line construct) followed
    // by an empty continuation, and a doubly-nested empty continuation.
    assert_no_duplicate_content("> ---\n> x");
    assert_no_duplicate_content(">===\n>");
    assert_no_duplicate_content("> a\n> ---\n>");
    assert_no_duplicate_content("> ---\n>>");
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
                .map(|l| l.spans.iter().map(|s| s.buffer_end.saturating_sub(s.buffer_start)).sum())
                .unwrap_or(0);
            let hidden = snap.hidden_byte_count(line);
            prop_assert_eq!(visible + hidden, expected_len, "line {} coverage gap: visible {} + hidden {} != length {}", line, visible, hidden, expected_len);

            // When a line has NO hidden ranges at all (nothing on it is
            // concealed — note this is a different question from "every
            // span.state == Revealed": a Text run nested inside a
            // CONCEALED emphasis is still tagged Revealed itself, since
            // `state` marks "this run is a verbatim buffer copy", not
            // "nothing wrapping it is hidden" — only `hidden_byte_count`
            // answers the line-wide question), the concatenated span text
            // must equal the exact buffer line bytes.
            if let Some(l) = lines.get(line)
                && !l.spans.is_empty()
                && hidden == 0
            {
                let joined: String = l.spans.iter().map(|s| s.text.as_str()).collect();
                prop_assert_eq!(joined, buf.line(line));
            }
        }
    }
}
