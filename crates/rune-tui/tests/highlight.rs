//! WP5.S7: `Msg::Highlighted` semantics — keep-on-`None` (`[R2]`),
//! drop-on-stale-version, and clamp-plus-char-boundary-discard on receipt
//! (§1.3) — plus one end-to-end highlight-then-render check. The painter
//! resolution itself (a hand-built `(rows, spans)` pair, outer-first
//! overwrite, `buf_offset`/`width` left untouched) is unit-tested inside
//! `render/overlay.rs`'s own `#[cfg(test)]` module instead: its target,
//! `apply_highlight_spans`, is `pub(super)` like every other overlay
//! function in that file (`apply_cursor_overlays`, `highlight_selection`,
//! `place_caret`), so it is unreachable from this external integration test
//! crate — only the crate's own public surface (`app::update`, `Document`,
//! `render::build_rows`/`testgrid`) is.
//!
//! WP6.S5 adds the fenced-code-in-markdown end-to-end cases below: a
//! markdown document's own fences schedule and apply through the SAME
//! `Msg::Highlighted`/`update`/`testgrid` path as a whole code document —
//! `crate::highlight::schedule_highlight`, `fence_language` and
//! `code_fence_sources` are private to `rune-tui`, so these drive the real
//! public chokepoints (`app::update`, `Cmd::run`) instead of calling them
//! directly. `clippy::panic` joins the allow list here (matching
//! `tests/opentabs.rs`/`tests/db_wiring.rs`/`tests/rename.rs`/
//! `tests/banner.rs`'s own convention) for the "wrong Msg variant landed"
//! assertions those new cases need.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_syntax::ScopeId;
use rune_syntax::scope::scope_table;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, HighlightPayload, Msg};
use rune_tui::testgrid;
use rune_vfs::Mem;

fn app_for(content: &str, path: &str) -> App {
    App::new(
        Buffer::new(content),
        Some(PathBuf::from(path)),
        Arc::new(Mem::new()),
        None,
    )
}

/// Types one harmless character at the END of the active document's buffer
/// (plan WP6.S5) through the real `app::update` chokepoint, bumping its
/// buffer version so `App::update`'s own before/after gate schedules a
/// highlight `Cmd` — mirrors how a real keystroke does it, without needing
/// the private `highlight::schedule_highlight` directly. The cursor is moved
/// to the very end first (rather than wherever `App::new` put it) so the
/// edit is a pure append. Scheduling refreshes the block tree itself, so an
/// edit BEFORE a fence is equally safe — the sibling regression test below
/// covers exactly that.
fn type_one_char_at_end(app: &mut App, effects: &mut Effects) {
    let id = app.active;
    let end = app.doc(id).expect("doc").buffer.content().len();
    app.doc_mut(id).expect("doc").cursors = CursorSet::new(end);
    app::update(
        app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('!'),
            mods: Mods::NONE,
        }),
        effects,
    );
}

#[test]
fn none_result_leaves_spans_byte_identical() {
    let mut app = app_for("fn main() {}\n", "/x/main.rs");
    let id = app.active;
    let keyword = scope_table().resolve("keyword").expect("known scope");
    let before = vec![(0..2, keyword)];
    app.doc_mut(id).expect("doc").highlight.spans = before.clone();
    let version = app.doc(id).expect("doc").buffer.version();

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version,
            result: None,
        },
        &mut effects,
    );

    assert_eq!(app.doc(id).expect("doc").highlight.spans, before);
}

#[test]
fn reply_at_a_stale_version_leaves_spans_unchanged() {
    let mut app = app_for("fn main() {}\n", "/x/main.rs");
    let id = app.active;
    let keyword = scope_table().resolve("keyword").expect("known scope");
    let before = vec![(0..2, keyword)];
    app.doc_mut(id).expect("doc").highlight.spans = before.clone();
    let stale_version = app.doc(id).expect("doc").buffer.version();

    // Advance the buffer past `stale_version` without going through a real
    // edit command — a direct field write is the same convention
    // `tests/tui_render.rs::app_for` already uses for other `Document`
    // fields (`cursors`, `viewport`).
    {
        let doc = app.doc_mut(id).expect("doc");
        doc.buffer = doc.buffer.insert(0, "x");
    }

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version: stale_version,
            result: Some(HighlightPayload::Spans(vec![(0..3, keyword)].into())),
        },
        &mut effects,
    );

    assert_eq!(app.doc(id).expect("doc").highlight.spans, before);
}

#[test]
// The `5..3` payload below is a deliberately inverted range — exactly the
// hostile reply `handle_highlighted` must discard — not an accidental
// reversed-iteration mistake.
#[allow(clippy::reversed_empty_ranges)]
fn clamps_and_drops_out_of_bounds_and_off_char_boundary_ranges() {
    // 3 CJK codepoints (3 bytes each) + `\n` = 10 bytes; byte 1 sits inside
    // the first codepoint, never on a `char` boundary.
    let content = "日本語\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;
    let len = app.doc(id).expect("doc").buffer.content().len();
    assert_eq!(len, 10);
    let version = app.doc(id).expect("doc").buffer.version();
    let keyword = scope_table().resolve("keyword").expect("known scope");

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version,
            result: Some(HighlightPayload::Spans(
                vec![
                    (0..1000, keyword), // past the end -> clamped to `len`
                    (5..3, keyword),    // inverted -> dropped
                    (1..2, keyword),    // mid-char -> dropped
                    (0..3, keyword),    // valid, char-boundary aligned
                ]
                .into(),
            )),
        },
        &mut effects,
    );

    let doc = app.doc(id).expect("doc");
    let content = doc.buffer.content();
    for (range, _) in &doc.highlight.spans {
        assert!(range.start < range.end);
        assert!(range.end <= content.len());
        assert!(content.is_char_boundary(range.start));
        assert!(content.is_char_boundary(range.end));
    }
    assert!(doc.highlight.spans.contains(&(0..len, keyword)));
    assert!(doc.highlight.spans.contains(&(0..3, keyword)));
    assert_eq!(
        doc.highlight.spans.len(),
        2,
        "the inverted and mid-char ranges must be dropped"
    );
}

#[test]
fn a_real_highlight_reply_colours_a_code_document_without_changing_its_text() {
    let content = "fn main() {}\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;
    app.doc_mut(id).expect("doc").viewport.set_size(40, 10);
    let version = app.doc(id).expect("doc").buffer.version();

    let result = rune_ts::highlight("rust", content, Duration::from_secs(5));
    assert!(
        result.is_some(),
        "a trivial rust source must highlight within a generous budget"
    );

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version,
            result: result.map(HighlightPayload::Spans),
        },
        &mut effects,
    );

    app.sync_view();
    let rendered = testgrid::grid(&app, 40, 10).join("\n");
    assert!(
        rendered.contains("fn main() {}"),
        "the overlay must never change the rendered TEXT, only cell styles"
    );

    let buf = testgrid::draw(&app, 40, 10);
    let plain = app
        .theme
        .scope_style(scope_table().resolve("text").expect("known scope"));
    let mut any_non_plain = false;
    for y in 0..10 {
        for x in 0..40 {
            if let Some(cell) = buf.cell((x, y))
                && cell.style().fg != plain.fg
            {
                any_non_plain = true;
            }
        }
    }
    assert!(
        any_non_plain,
        "at least one cell must carry a token colour distinct from the plain text scope"
    );
}

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

/// Plan WP6.S5, bullet 2: rendering a fenced markdown document leaves
/// every cell inside the fence carrying a non-`None` background — the
/// `markup.raw.block` fence background survives the highlight overlay's
/// `patch` (plan decision 2), since `code_scope_style` sets only `fg` and
/// never `.bg(..)` (WP1.S4).
#[test]
fn fence_background_survives_the_highlight_overlay_patch() {
    let content = "Intro paragraph.\n\n```rust\nfn main() {}\n```\n\nOutro.\n";
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();
    app.doc_mut(app.active)
        .expect("doc")
        .viewport
        .set_size(40, 12);

    let mut effects = Effects::default();
    type_one_char_at_end(&mut app, &mut effects);
    let msg = effects
        .cmds
        .remove(0)
        .run()
        .expect("fence_highlight_cmd always replies");
    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);

    app.sync_view();
    let buf = testgrid::draw(&app, 40, 12);
    let raw_block_bg = app
        .theme
        .scope_style(
            scope_table()
                .resolve("markup.raw.block")
                .expect("known scope"),
        )
        .bg;
    assert!(
        raw_block_bg.is_some(),
        "the fence background style itself must carry a bg"
    );

    // Scoped to the fence TEXT's own columns, not the whole row: `geo.
    // center_bordered`'s rounded-border box shares the row with the
    // editor's content on a narrow frame, and that border cell's style is
    // chrome, not `markup.raw.block` — checking the whole row would assert
    // an unrelated cell instead of the fence's own. Matched cell-by-cell
    // (not via `String::find`) because the border glyph is a multi-BYTE
    // UTF-8 char while a terminal column is one CELL — `find`'s byte
    // offset and the column index silently diverge the moment a
    // multi-byte cell precedes the match.
    let needle: Vec<char> = "fn main() {}".chars().collect();
    let mut found_fence_row = false;
    for y in 0..12u16 {
        for x0 in 0..40u16 {
            let matched = needle.iter().enumerate().all(|(k, &nc)| {
                let x = x0 + u16::try_from(k).unwrap_or(u16::MAX);
                buf.cell((x, y))
                    .is_some_and(|cell| cell.symbol() == nc.to_string())
            });
            if !matched {
                continue;
            }
            found_fence_row = true;
            for k in 0..needle.len() {
                let x = x0 + u16::try_from(k).unwrap_or(u16::MAX);
                let cell = buf.cell((x, y)).expect("just matched above");
                assert_eq!(
                    cell.style().bg,
                    raw_block_bg,
                    "fence content cell at column {x} lost its markup.raw.block background"
                );
            }
        }
    }
    assert!(found_fence_row, "the fence content row must be on screen");
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

/// D5: a `None` reply for a scheduled CODE document (a whole-document parse
/// that timed out, hit an unresolvable language, or failed) surfaces the
/// same status line finding B's exhausted-retry branch used to — in ONE
/// attempt, since D5 replaces the whole retry chain with a single bounded
/// `PARSE_BUDGET` parse. `doc.kind.language().is_some()` is the gate
/// `handle_highlighted` uses to tell a code document's parse reply from a
/// markdown document's fence reply.
#[test]
fn a_timed_out_code_document_surfaces_a_status_message() {
    let content = "fn main() {}\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;
    let version = app.doc(id).expect("doc").buffer.version();
    assert_eq!(
        app.doc(id).expect("doc").highlight.version,
        0,
        "a fresh document must never have been highlighted yet"
    );

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version,
            result: None,
        },
        &mut effects,
    );

    let doc = app.doc(id).expect("doc");
    assert!(
        doc.highlight.spans.is_empty() && doc.highlight.tree.is_none(),
        "a None reply must never invent spans or a tree"
    );
    assert_eq!(
        doc.highlight.in_flight, None,
        "in_flight must still clear on a timed-out reply, or this document \
         could never be highlighted again by any future edit"
    );
    assert!(
        effects.cmds.is_empty(),
        "a single-attempt timeout schedules no further cmd — there is no \
         retry chain to continue"
    );
    assert_eq!(
        app.status_message.as_deref(),
        Some("syntax highlighting timed out for this document"),
        "a timed-out CODE document's parse must surface a status line, not \
         fail silently"
    );
}

/// The sibling of the case above: a `None` reply for a MARKDOWN document
/// (its fences failed/timed out, never a whole-document parse) must stay
/// silent exactly as `[R2]` already requires for any other stale/failed
/// reply — D6 leaves the fence pipeline's `None` handling untouched.
#[test]
fn a_timed_out_markdown_fence_reply_stays_silent() {
    let content = "```rust\nfn main() {}\n```\n";
    let mut app = app_for(content, "/x/notes.md");
    let id = app.active;
    let version = app.doc(id).expect("doc").buffer.version();

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version,
            result: None,
        },
        &mut effects,
    );

    assert_eq!(
        app.status_message, None,
        "a markdown document's fence timeout must not surface the \
         code-document status line"
    );
}

/// D6: a `Tree` payload applied to a live-version reply stores the tree and
/// stamps `highlight.version`, mirroring what the old span-clamp path did
/// for `Spans` — the field the render path (`render::build_rows`) and
/// `highlight::schedule_highlight`'s already-current guard both read.
#[test]
fn a_tree_payload_populates_the_retained_tree() {
    let content = "fn main() {}\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;
    let version = app.doc(id).expect("doc").buffer.version();

    let tree = rune_ts::parse("rust", content, Duration::from_secs(5))
        .expect("a trivial rust source must parse within a generous budget");

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version,
            result: Some(HighlightPayload::Tree(tree)),
        },
        &mut effects,
    );

    let doc = app.doc(id).expect("doc");
    assert!(doc.highlight.tree.is_some(), "the tree must be stored");
    assert_eq!(doc.highlight.version, version);
}
