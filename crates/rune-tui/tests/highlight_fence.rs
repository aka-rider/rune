//! Split off `highlight.rs` (WP11, §1.6): WP6.S5's fenced-code-in-markdown
//! end-to-end cases — a markdown document's own fences schedule and apply
//! through the SAME `Msg::Highlighted`/`update`/`testgrid` path as a whole
//! code document — `crate::highlight::schedule_highlight`, `fence_language`
//! and `code_fence_sources` are private to `rune-tui`, so these drive the
//! real public chokepoints (`app::update`, `Cmd::run`) instead of calling
//! them directly. `clippy::panic` joins the allow list here (matching
//! `tests/opentabs.rs`/`tests/db_wiring.rs`/`tests/rename.rs`/
//! `tests/banner.rs`'s own convention) for the "wrong Msg variant landed"
//! assertions these cases need.
//!
//! Also carries the container-prefix-leak ("finding A") cases: a fence
//! nested in a blockquote or list item must not feed the container's own
//! repeating marker prefix to `rune_ts::highlight` as source bytes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod highlight_common;

use std::ops::Range;

use highlight_common::{app_for, type_one_char_at_end};
use ratatui::buffer::Buffer as RtBuffer;
use ratatui::style::{Modifier, Style};
use rune_core::cursor::CursorSet;
use rune_syntax::ScopeId;
use rune_syntax::scope::scope_table;
use rune_tui::app;
use rune_tui::runtime::{Effects, HighlightPayload, Msg};
use rune_tui::testgrid;

/// Plan WP6.S5, bullet 1: a markdown document with one ```` ```rust ````
/// fence produces at least one stored span inside the fence's own content
/// bytes and none outside it.
#[test]
fn markdown_rust_fence_produces_spans_inside_the_fence_only() {
    let content = "Intro paragraph.\n\n```rust\nfn main() {}\n```\n\nOutro.\n";
    let mut app = app_for(content, "/x/notes.md");
    // Mirrors `runtime::run`'s own bootstrap ordering (plan Context, "the
    // async seam"): `DocMachine::code_fences` reads the LAST parse
    // `sync_view` produced, not the live buffer, so a fence must have been
    // parsed at least once before an edit can find it.
    app.sync_view();

    let mut effects = Effects::default();
    type_one_char_at_end(&mut app, &mut effects);

    assert_eq!(
        effects.cmds.len(),
        1,
        "the rust fence must schedule exactly one highlight cmd"
    );
    let msg = effects
        .cmds
        .remove(0)
        .run()
        .expect("fence_highlight_cmd always replies with Some(Msg::Highlighted)");
    let Msg::Highlighted { result, .. } = &msg else {
        panic!("expected a Msg::Highlighted reply, got {msg:?}");
    };
    let spans = match result {
        Some(HighlightPayload::Spans(spans)) => spans.clone(),
        other => panic!("expected a Spans payload, got {other:?}"),
    };
    assert!(!spans.spans.is_empty());

    let fence_start = content.find("fn main").expect("fixture has a fence body");
    let fence_end = content
        .find("```\n\nOutro")
        .expect("fixture has a fence close");
    for (range, _) in &spans.spans {
        assert!(
            range.start >= fence_start && range.end <= fence_end,
            "span {range:?} escapes the fence content bytes {fence_start}..{fence_end}"
        );
    }

    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);
    let doc = app.doc(app.active).expect("doc");
    assert!(!doc.highlight.spans.is_empty());
    for (range, _) in &doc.highlight.spans {
        assert!(range.start >= fence_start && range.end <= fence_end);
    }
}

/// Plan WP6.S5, bullet 3: a fence tagged ```` ```rust,ignore ```` still
/// resolves to `rust` (info string split on whitespace AND `,`, first
/// token only) and still produces spans.
#[test]
fn fence_tagged_rust_comma_ignore_still_highlights() {
    let content = "```rust,ignore\nfn main() {}\n```\n";
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();

    let mut effects = Effects::default();
    type_one_char_at_end(&mut app, &mut effects);

    assert_eq!(
        effects.cmds.len(),
        1,
        "rust,ignore must still resolve and schedule a highlight cmd"
    );
    let msg = effects.cmds.remove(0).run().expect("reply always arrives");
    let Msg::Highlighted { result, .. } = &msg else {
        panic!("expected a Msg::Highlighted reply, got {msg:?}");
    };
    assert!(
        matches!(result, Some(HighlightPayload::Spans(r)) if !r.spans.is_empty()),
        "a rust,ignore fence must still produce spans"
    );
}

/// Plan WP6.S5, bullet 4: an unknown fence tag and an untagged fence each
/// produce zero spans and no error — neither resolves through
/// `rune_ts::lang::resolve`, so `code_fence_sources` returns nothing and
/// `schedule_highlight`'s markdown branch schedules no `Cmd` at all.
#[test]
fn unknown_and_untagged_fences_schedule_nothing() {
    let content = "```klingon\nQapla'\n```\n\n```\nplain fenced text\n```\n";
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();

    let mut effects = Effects::default();
    type_one_char_at_end(&mut app, &mut effects);

    assert!(
        effects.cmds.is_empty(),
        "no fence resolved to a known language, so no highlight cmd should be scheduled"
    );
    assert!(app.doc(app.active).expect("doc").highlight.spans.is_empty());
}

/// An edit that lands BEFORE a fence must not leave that fence's spans at
/// their pre-edit offsets. Scheduling runs inside the update loop, while the
/// settle step that rebuilds the block tree runs after it returns — so the
/// fence ranges a scheduled command reads are the previous version's unless
/// scheduling refreshes them first. The reply carries the CURRENT version, so
/// the staleness check accepts it and every fence would be painted shifted by
/// the edit's own delta, with nothing rescheduling until the next keystroke.
#[test]
fn an_edit_before_a_fence_does_not_shift_its_spans() {
    let content = "Intro paragraph.\n\n```rust\nfn main() {}\n```\n\nOutro.\n";
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();

    // Insert ahead of the fence in ONE edit, and insert enough bytes that a
    // stale range cannot accidentally still work: with a single byte the two
    // errors cancel (the slice starts one byte early, so its tokens sit one
    // byte later inside it, and rebasing by the stale start cancels out).
    // A wider shift moves the stale window off the fence body entirely.
    let id = app.active;
    app.doc_mut(id).expect("doc").cursors = CursorSet::new(0);
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Paste("a much longer prefix inserted ahead of the fence\n\n".to_string()),
        &mut effects,
    );

    let cmd = effects
        .cmds
        .pop()
        .expect("an edit before the fence must still schedule a highlight");
    let msg = cmd.run().expect("fence_highlight_cmd always replies");
    let mut effects = Effects::default();
    app::update(&mut app, msg, &mut effects);

    let doc = app.doc(id).expect("doc");
    let updated = doc.buffer.content().to_string();
    let fence_start = updated.find("fn main").expect("fence body");
    let fence_end = updated[fence_start..]
        .find("```")
        .map(|i| fence_start + i)
        .expect("fence close");

    assert!(
        !doc.highlight.spans.is_empty(),
        "the rust fence must still produce spans after a leading insert"
    );
    for (range, _) in &doc.highlight.spans {
        assert!(
            range.start >= fence_start && range.end <= fence_end,
            "span {range:?} is outside the fence's post-edit bytes \
             {fence_start}..{fence_end}"
        );
    }

    // Containment alone is too weak to catch a stale parse: a one-byte shift
    // still lands inside the fence. Compare the bytes a span actually selects
    // against the token they must select — off by one and this reads "\nf".
    let sliced: Vec<&str> = doc
        .highlight
        .spans
        .iter()
        .filter_map(|(range, _)| updated.get(range.clone()))
        .collect();
    assert!(
        sliced.contains(&"fn"),
        "no span selects the `fn` keyword exactly; spans selected {sliced:?} \
         — they were rebased onto a pre-edit parse of the fence"
    );
}

/// Schedules and runs the ONE highlight `Cmd` a fresh markdown document
/// with exactly one resolvable fence produces (mirrors the setup every test
/// above this point repeats), and returns the spans it replied with —
/// panicking with a descriptive message if scheduling or parsing didn't
/// happen the way every other case here expects. Used by the finding-A
/// (container-prefix leak) cases below, which only care about the reply's
/// spans, never about applying them to a `Document`.
fn fence_highlight_spans(content: &str, path: &str) -> Vec<(Range<usize>, ScopeId)> {
    let mut app = app_for(content, path);
    app.sync_view();
    let mut effects = Effects::default();
    type_one_char_at_end(&mut app, &mut effects);
    assert_eq!(
        effects.cmds.len(),
        1,
        "expected exactly one scheduled highlight cmd for {content:?}"
    );
    let msg = effects
        .cmds
        .remove(0)
        .run()
        .expect("fence_highlight_cmd always replies with Some(..)");
    let Msg::Highlighted { result, .. } = msg else {
        panic!("expected a Msg::Highlighted reply, got {msg:?}");
    };
    match result.expect("the fence must parse within the budget") {
        HighlightPayload::Spans(spans) => spans.spans,
        HighlightPayload::Tree(_) => panic!("a fence reply must never carry a Tree payload"),
    }
}

/// Finding A: a fence nested in a blockquote must not feed the
/// blockquote's own repeating `"> "` prefix to `rune_ts::highlight` as
/// source bytes. YAML is the language the investigation measured this on
/// (14 spans clean vs. 3 once `"> "` starts corrupting indentation-
/// sensitive structure) — a top-level fence and the byte-identical fence
/// nested one blockquote level deep must produce the exact same span
/// count, since after `code_fence_sources` reconstructs the prefix-free
/// source, tree-sitter sees byte-identical text either way.
#[test]
fn yaml_fence_nested_in_blockquote_produces_the_same_span_count_as_top_level() {
    let yaml_body = "key: value\nnested:\n  child: 1\nlist:\n  - a\n  - b";

    let top_level = format!("```yaml\n{yaml_body}\n```\n");
    let top_spans = fence_highlight_spans(&top_level, "/x/top.md");
    assert!(
        !top_spans.is_empty(),
        "a top-level yaml fence must produce spans at all"
    );

    // Every line, including the fence markers, gets the SAME "> " prefix —
    // stripping it must reconstruct source byte-identical to `top_level`'s
    // own fence content.
    let quoted_body: String = yaml_body
        .lines()
        .map(|line| format!("> {line}\n"))
        .collect();
    let nested = format!("> ```yaml\n{quoted_body}> ```\n");
    let nested_spans = fence_highlight_spans(&nested, "/x/nested.md");

    assert_eq!(
        nested_spans.len(),
        top_spans.len(),
        "a blockquoted yaml fence must highlight identically to the same \
         fence at top level — a span-count mismatch means the blockquote's \
         own \"> \" prefix leaked into the parsed source \
         (top: {top_spans:?}, nested: {nested_spans:?})"
    );
}

/// Finding A, list-item variant: the same prefix-leak bug applies to a
/// list item's own repeating indent, not just a blockquote's `"> "`. Rust's
/// error recovery absorbs a stray `"> "` (the investigation's own measured
/// table), so this uses a structured-enough fixture (a function with a
/// nested statement) that a shifted/corrupted source would still visibly
/// change the span count, not silently reparse to the same shape.
#[test]
fn rust_fence_nested_in_list_item_produces_the_same_span_count_as_top_level() {
    let top_level = "```rust\nfn main() {\n    let a = 1;\n}\n```\n";
    let top_spans = fence_highlight_spans(top_level, "/x/top.md");
    assert!(
        !top_spans.is_empty(),
        "a top-level rust fence must produce spans at all"
    );

    // A "- " marker is 2 bytes wide, so CommonMark requires every
    // continuation line (the fence markers AND its content) indented by
    // exactly 2 spaces — stripping that indent must reconstruct source
    // byte-identical to `top_level`'s own fence content.
    let nested = "- ```rust\n  fn main() {\n      let a = 1;\n  }\n  ```\n";
    let nested_spans = fence_highlight_spans(nested, "/x/nested.md");

    assert_eq!(
        nested_spans.len(),
        top_spans.len(),
        "a rust fence nested in a list item must highlight identically to \
         the same fence at top level (top: {top_spans:?}, nested: {nested_spans:?})"
    );
}

/// Finding A: containment alone ("the span is somewhere inside the fence's
/// buffer bytes") is too weak — a span could still straddle or sit right on
/// top of the blockquote's own `"> "` marker bytes and merely happen to
/// stay within the fence's overall extent. This asserts on the EXACT bytes
/// each span selects: every one must be a real token, and none may begin
/// with (or contain) a literal `"> "` — the blockquote marker this nested
/// fence's every continuation line carries. YAML, not rust, is deliberately
/// chosen here: rust's error recovery absorbs a stray `"> "` so completely
/// that no span ever lands on it either way (the investigation's own
/// measured table), which would make this assertion true regardless of
/// whether the underlying fix is present — exactly the containment-only
/// trap this test exists to avoid falling into a second time.
#[test]
fn nested_fence_spans_never_select_the_blockquote_prefix_bytes() {
    let content = "> ```yaml\n> key: value\n> nested:\n>   child: 1\n> ```\n";
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();

    let mut effects = Effects::default();
    type_one_char_at_end(&mut app, &mut effects);
    let msg = effects
        .cmds
        .remove(0)
        .run()
        .expect("fence_highlight_cmd always replies");
    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);

    let doc = app.doc(app.active).expect("doc");
    assert!(
        !doc.highlight.spans.is_empty(),
        "the blockquoted yaml fence must still produce spans"
    );

    let sliced: Vec<&str> = doc
        .highlight
        .spans
        .iter()
        .filter_map(|(range, _)| content.get(range.clone()))
        .collect();
    for text in &sliced {
        assert!(
            !text.starts_with("> ") && !text.contains("> "),
            "span text {text:?} carries the blockquote's own \"> \" marker \
             bytes — spans selected {sliced:?}"
        );
    }
    // Exact-token assertions (containment alone would pass even for a
    // shifted parse that happens to still land inside the fence): the
    // fence's real tokens must be selected verbatim.
    assert!(
        sliced.contains(&"key"),
        "no span selects the `key` mapping key exactly; spans selected {sliced:?}"
    );
    assert!(
        sliced.contains(&"nested"),
        "no span selects the `nested` mapping key exactly; spans selected {sliced:?}"
    );
    assert!(
        sliced.contains(&"child"),
        "no span selects the `child` mapping key exactly; spans selected {sliced:?}"
    );
}

/// Scans every cell of `buf` (`w` x `h`) for the first place `needle`
/// appears as a run of consecutive single-cell glyphs, and returns that
/// first cell's style — cell-by-cell (never `String::find` on a joined
/// row), matching `highlight_overlay.rs`'s own fence-cell search: a
/// multi-byte UTF-8 glyph occupies one terminal CELL, so a byte-offset
/// search and a column index silently diverge the moment one precedes the
/// match.
fn find_needle_style(buf: &RtBuffer, w: u16, h: u16, needle: &str) -> Option<Style> {
    let chars: Vec<char> = needle.chars().collect();
    for y in 0..h {
        for x0 in 0..w {
            let matched = chars.iter().enumerate().all(|(k, &nc)| {
                let x = x0 + u16::try_from(k).unwrap_or(u16::MAX);
                buf.cell((x, y))
                    .is_some_and(|cell| cell.symbol() == nc.to_string())
            });
            if matched {
                let cell = buf.cell((x0, y))?;
                return Some(cell.style());
            }
        }
    }
    None
}

/// Plan WP6.S5: a ```` ```markdown ```` fence (FOUR backticks, so its own
/// nested three-backtick fence doesn't close it early) gets INLINE markdown
/// highlighting through the comrak reveal-emit reuse path
/// (`runtime::md_fence::markdown_fence_spans`), not flat near-black text.
/// Because `reveal_all` forces every block revealed, the fence's own
/// contents render with their raw markdown markers visible (`# `, `**`,
/// `` ` ``, `[]()`), matching what a real revealed line would show. The
/// heading/bold/code/link ranges must carry their own markdown scopes'
/// styles OVER the fence's `markup.raw.block` background (WP1.S4's overlay
/// bg-strip is what lets that background survive the overlay patch), and
/// the nested three-backtick fence's own body must keep that SAME
/// background untouched (its lines resolve to `markup.raw.block` too, so
/// the overlay adds no bg change there either).
#[test]
fn markdown_fence_highlights_inline_markdown_over_the_fence_background() {
    let content = concat!(
        "Intro paragraph.\n",
        "\n",
        "````markdown\n",
        "# Title\n",
        "\n",
        "**bold** `snippet` [linktext](http://example.com)\n",
        "\n",
        "```rust\n",
        "fn main() {}\n",
        "```\n",
        "````\n",
        "\n",
        "Outro.\n",
    );
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();
    app.doc_mut(app.active)
        .expect("doc")
        .viewport
        .set_size(60, 20);

    let mut effects = Effects::default();
    type_one_char_at_end(&mut app, &mut effects);
    assert_eq!(
        effects.cmds.len(),
        1,
        "expected exactly one scheduled highlight cmd"
    );
    let msg = effects
        .cmds
        .remove(0)
        .run()
        .expect("fence_highlight_cmd always replies");
    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);

    app.sync_view();
    let buf = testgrid::draw(&app, 60, 20);

    let heading_style = app.theme.scope_style(
        scope_table()
            .resolve("markup.heading.1")
            .expect("known scope"),
    );
    let raw_inline_style = app.theme.scope_style(
        scope_table()
            .resolve("markup.raw.inline")
            .expect("known scope"),
    );
    let link_style = app
        .theme
        .scope_style(scope_table().resolve("markup.link").expect("known scope"));
    let fence_bg = app
        .theme
        .scope_style(
            scope_table()
                .resolve("markup.raw.block")
                .expect("known scope"),
        )
        .bg;
    assert!(
        fence_bg.is_some(),
        "the fence background style itself must carry a bg"
    );

    let title = find_needle_style(&buf, 60, 20, "Title").expect("heading text must be on screen");
    assert_eq!(
        title.fg, heading_style.fg,
        "heading text inside the markdown fence must carry the heading fg"
    );
    assert_eq!(
        title.bg, fence_bg,
        "heading text must still sit on the fence's own background"
    );

    let bold = find_needle_style(&buf, 60, 20, "bold").expect("bold text must be on screen");
    assert!(
        bold.add_modifier.contains(Modifier::BOLD),
        "bold text inside the markdown fence must carry the BOLD modifier"
    );
    assert_eq!(
        bold.bg, fence_bg,
        "bold text must still sit on the fence's own background"
    );

    let code =
        find_needle_style(&buf, 60, 20, "snippet").expect("inline-code text must be on screen");
    assert_eq!(
        code.fg, raw_inline_style.fg,
        "inline-code text inside the markdown fence must carry the raw.inline fg"
    );

    let link = find_needle_style(&buf, 60, 20, "linktext").expect("link text must be on screen");
    assert_eq!(
        link.fg, link_style.fg,
        "link text inside the markdown fence must carry the link fg"
    );
    assert!(
        link.add_modifier.contains(Modifier::UNDERLINED),
        "link text inside the markdown fence must carry the UNDERLINED modifier"
    );

    let inner_fence_body =
        find_needle_style(&buf, 60, 20, "fn main").expect("nested fence body must be on screen");
    assert_eq!(
        inner_fence_body.bg, fence_bg,
        "the nested three-backtick fence's own body must keep the outer fence background"
    );
}
