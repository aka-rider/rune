//! What one highlight pipeline means, stated as a specification: a fence
//! inside a markdown document and a whole source file are the same thing —
//! a code region — and must colour identically.
//!
//! `highlight::schedule_highlight` and the region resolution behind it are
//! private to `rune-tui`, so these drive the real public chokepoints
//! (`app::update`, `Cmd::run`, `highlight::visible_spans`) instead of
//! calling them directly. `clippy::panic` joins the allow list for the
//! "wrong Msg variant landed" assertions.
//!
//! The container-prefix-leak cases — a fence nested in a blockquote or list
//! item must not feed the container's own repeating marker prefix to the
//! parser as source bytes — live in the `_nesting` sibling (§1.6).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod highlight_common;

use highlight_common::{all_spans, app_for, region_tree_source, type_one_char_at_end};
use rune_core::cursor::CursorSet;
use rune_tui::app::{self, App};
use rune_tui::runtime::{Effects, Msg};

/// Runs the document's pending highlight to completion through the real
/// message path: schedule (by typing one character), run the `Cmd` inline,
/// deliver its reply. The state it leaves behind is read back through
/// `all_spans`, the same query the renderer uses.
fn settle_highlight(app: &mut App) {
    let mut effects = Effects::default();
    type_one_char_at_end(app, &mut effects);
    assert_eq!(
        effects.cmds.len(),
        1,
        "expected exactly one scheduled highlight cmd"
    );
    let msg = effects
        .cmds
        .remove(0)
        .run()
        .expect("a highlight cmd always replies with Some(Msg::Highlighted)");
    let Msg::Highlighted { .. } = &msg else {
        panic!("expected a Msg::Highlighted reply, got {msg:?}");
    };
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
}

/// Schedules a highlight by inserting `text` at `at` — a version bump the
/// caller controls, unlike the append `type_one_char_at_end` performs — and
/// settles the reply.
fn settle_after_insert(app: &mut App, at: usize, text: &str) {
    let id = app.active;
    app.doc_mut(id).expect("doc").cursors = CursorSet::new(at);
    let mut effects = Effects::default();
    app::update(app, Msg::Paste(text.to_string()), &mut effects);
    for cmd in effects.cmds.drain(..) {
        if let Some(msg) = cmd.run() {
            let mut settled = Effects::default();
            app::update(app, msg, &mut settled);
        }
    }
}

/// Every span a settled document would paint, as the exact source text each
/// one selects. Comparing TEXT rather than offsets is what lets the same
/// code be compared across two documents that place it at different buffer
/// positions.
fn settled_span_texts(content: &str, path: &str, code_at: usize) -> Vec<String> {
    let mut app = app_for(content, path);
    app.sync_view();
    // The version bump lands INSIDE the code in both documents, so the two
    // parsers see byte-identical source — the comparison is about the
    // pipeline, not about what an edit outside a fence does to it.
    settle_after_insert(&mut app, code_at, "\n");
    let doc_content = app.active_doc().buffer.content().to_string();
    all_spans(&app)
        .into_iter()
        .filter_map(|(range, _)| doc_content.get(range).map(str::to_string))
        .collect()
}

/// THE point of collapsing the two pipelines: identical code inside a fence
/// and inside a whole source file produces identical spans. Before, a fence
/// and a file disagreed on budget, retention, query scope and clamping, so
/// the same bytes could render differently depending only on where they sat.
#[test]
fn a_fence_and_a_file_produce_the_same_spans_for_the_same_code() {
    let code = "fn main() {\n    let a = 1;\n}\n";
    let file = settled_span_texts(code, "/x/main.rs", 0);
    let markdown = format!("```rust\n{code}```\n");
    let fence_at = markdown.find("fn main").expect("fixture has a fence body");
    let fence = settled_span_texts(&markdown, "/x/notes.md", fence_at);

    assert!(!file.is_empty(), "a rust file must produce spans at all");
    assert_eq!(
        fence, file,
        "the same code must select the same tokens in a fence as in a file"
    );
}

/// The regression the divided budget caused: a document with many fences got
/// one quarter-second SPLIT between them, so a later fence could silently
/// render flat. Every fence must now highlight.
#[test]
fn every_fence_in_a_many_fence_document_highlights() {
    let langs = ["rust", "python", "go", "yaml", "json", "toml"];
    let mut content = String::new();
    for (i, lang) in langs.iter().enumerate() {
        content.push_str(&format!("Prose paragraph {i}.\n\n"));
        content.push_str(&format!("```{lang}\n"));
        content.push_str(match *lang {
            "rust" => "fn main() { let a = 1; }\n",
            "python" => "def main():\n    a = 1\n",
            "go" => "package main\n\nfunc main() {}\n",
            "yaml" => "key: value\nnested:\n  child: 1\n",
            "json" => "{\"key\": \"value\"}\n",
            _ => "key = \"value\"\n",
        });
        content.push_str("```\n\n");
    }

    let mut app = app_for(&content, "/x/many.md");
    app.sync_view();
    settle_highlight(&mut app);

    let doc = app.active_doc();
    assert_eq!(
        doc.highlight.regions.len(),
        langs.len(),
        "every fence must become a region"
    );
    for (i, region) in doc.highlight.regions.iter().enumerate() {
        assert!(
            region.tree.is_some(),
            "fence {i} ({}) has no retained tree — it was starved of budget",
            langs[i]
        );
    }

    // Every fence's own bytes must actually be covered by the render
    // query's output, not merely have a tree behind them. Regions come in
    // document order, so each one's territory runs from its first content
    // byte up to the next region's — the fences are separated by prose, so
    // no span of one can be mistaken for a span of another.
    let starts: Vec<usize> = doc
        .highlight
        .regions
        .iter()
        .map(|region| {
            region
                .map
                .to_buffer(0..1)
                .expect("every region covers at least one byte")
                .start
        })
        .collect();
    let end_of_document = doc.buffer.content().len();
    let spans = all_spans(&app);
    for (i, start) in starts.iter().enumerate() {
        let limit = starts.get(i + 1).copied().unwrap_or(end_of_document);
        assert!(
            spans
                .iter()
                .any(|(range, _)| range.start >= *start && range.end <= limit),
            "fence {i} ({}) contributes no span in {start}..{limit} to the \
             render query — it was starved of budget",
            langs[i]
        );
    }
}

/// Tree reuse, the property that makes one full budget per region
/// affordable: editing prose BETWEEN two fences reparses neither of them.
/// Observed through the retained trees' own source snapshots — they must be
/// the very same parses, not fresh ones that happen to agree.
#[test]
fn editing_prose_between_two_fences_invalidates_neither_fence_tree() {
    let content = "```rust\nfn a() {}\n```\n\nprose\n\n```rust\nfn b() {}\n```\n";
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();
    settle_highlight(&mut app);

    assert_eq!(
        region_tree_source(&app, 0).as_deref(),
        Some("fn a() {}"),
        "the first fence must have parsed its own body"
    );
    assert_eq!(region_tree_source(&app, 1).as_deref(), Some("fn b() {}"));

    // Edit the prose paragraph sitting between the two fences. It shifts the
    // second fence's buffer offsets without changing either fence's text.
    let id = app.active;
    let at = content.find("prose").expect("fixture has prose");
    app.doc_mut(id).expect("doc").cursors = CursorSet::new(at);
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Paste("a much longer paragraph of prose ".to_string()),
        &mut effects,
    );

    assert!(
        effects.cmds.is_empty(),
        "an edit that leaves every fence's text alone must dispatch no \
         parse at all — both retained trees are still valid"
    );
    assert_eq!(
        app.active_doc().highlight.version,
        app.active_doc().buffer.version(),
        "the regions' maps must still have been refreshed to the new version"
    );

    // The colours must have MOVED with the text, not stayed at pre-edit
    // offsets: reusing a tree is only correct if the map was refreshed.
    let updated = app.active_doc().buffer.content().to_string();
    let texts: Vec<&str> = all_spans(&app)
        .into_iter()
        .filter_map(|(range, _)| updated.get(range))
        .collect();
    assert!(
        texts.contains(&"fn"),
        "no span selects the `fn` keyword exactly after the prose edit; \
         spans selected {texts:?}"
    );
}

/// A ```` ```markdown ```` fence has no tree-sitter grammar — it highlights
/// through the span channel instead, and must still colour.
#[test]
fn a_markdown_fence_highlights_through_the_span_channel() {
    let content = "```markdown\n# Title\n\nplain text\n```\n";
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();
    settle_highlight(&mut app);

    let doc = app.active_doc();
    let region = doc
        .highlight
        .regions
        .first()
        .expect("the markdown fence must become a region");
    assert!(
        region.tree.is_none(),
        "markdown stays comrak's — no tree-sitter grammar backs this fence"
    );
    assert!(
        !region.spans.is_empty(),
        "the span channel must carry the markdown fence's colours"
    );

    let heading = rune_syntax::scope::scope_table()
        .resolve("markup.heading.1")
        .expect("known scope");
    assert!(
        all_spans(&app).iter().any(|(_, scope)| *scope == heading),
        "the fenced heading must reach the render query"
    );
}

/// A markdown document with one ```` ```rust ```` fence produces at least
/// one span inside the fence's own content bytes and none outside it.
#[test]
fn markdown_rust_fence_produces_spans_inside_the_fence_only() {
    let content = "Intro paragraph.\n\n```rust\nfn main() {}\n```\n\nOutro.\n";
    let mut app = app_for(content, "/x/notes.md");
    // Mirrors `runtime::run`'s own bootstrap ordering: `DocMachine::
    // code_regions` reads the LAST parse `sync_view` produced, not the live
    // buffer, so a fence must have been parsed at least once before an edit
    // can find it.
    app.sync_view();
    settle_highlight(&mut app);

    let updated = app.active_doc().buffer.content().to_string();
    let fence_start = updated.find("fn main").expect("fixture has a fence body");
    let fence_end = updated
        .find("```\n\nOutro")
        .expect("fixture has a fence close");

    let spans = all_spans(&app);
    assert!(!spans.is_empty());
    for (range, _) in &spans {
        assert!(
            range.start >= fence_start && range.end <= fence_end,
            "span {range:?} escapes the fence content bytes {fence_start}..{fence_end}"
        );
    }
}

/// A fence tagged ```` ```rust,ignore ```` still resolves to `rust` (info
/// string split on whitespace AND `,`, first token only) and still produces
/// spans.
#[test]
fn fence_tagged_rust_comma_ignore_still_highlights() {
    let mut app = app_for("```rust,ignore\nfn main() {}\n```\n", "/x/notes.md");
    app.sync_view();
    settle_highlight(&mut app);

    assert!(
        !all_spans(&app).is_empty(),
        "a rust,ignore fence must still produce spans"
    );
}

/// An unknown fence tag and an untagged fence each produce zero spans and no
/// error — neither resolves to a highlighter, so neither becomes a region
/// and no `Cmd` is scheduled at all.
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
    assert!(app.active_doc().highlight.regions.is_empty());
    assert!(all_spans(&app).is_empty());
}

/// An edit that lands BEFORE a fence must not leave that fence's spans at
/// their pre-edit offsets. Scheduling runs inside the update loop, while the
/// settle step that rebuilds the block tree runs after it returns — so the
/// region ranges a scheduled command reads are the previous version's unless
/// scheduling refreshes them first.
#[test]
fn an_edit_before_a_fence_does_not_shift_its_spans() {
    let content = "Intro paragraph.\n\n```rust\nfn main() {}\n```\n\nOutro.\n";
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();
    settle_highlight(&mut app);

    // Insert ahead of the fence in ONE edit, and insert enough bytes that a
    // stale range cannot accidentally still work: with a single byte the two
    // errors cancel. A wider shift moves the stale window off the fence body
    // entirely.
    let id = app.active;
    app.doc_mut(id).expect("doc").cursors = CursorSet::new(0);
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Paste("a much longer prefix inserted ahead of the fence\n\n".to_string()),
        &mut effects,
    );
    for cmd in effects.cmds.drain(..) {
        if let Some(msg) = cmd.run() {
            let mut effects2 = Effects::default();
            app::update(&mut app, msg, &mut effects2);
        }
    }

    let updated = app.active_doc().buffer.content().to_string();
    let fence_start = updated.find("fn main").expect("fence body");
    let fence_end = updated[fence_start..]
        .find("```")
        .map(|i| fence_start + i)
        .expect("fence close");

    let spans = all_spans(&app);
    assert!(
        !spans.is_empty(),
        "the rust fence must still produce spans after a leading insert"
    );
    for (range, _) in &spans {
        assert!(
            range.start >= fence_start && range.end <= fence_end,
            "span {range:?} is outside the fence's post-edit bytes \
             {fence_start}..{fence_end}"
        );
    }

    // Containment alone is too weak to catch a stale parse: a one-byte shift
    // still lands inside the fence. Compare the bytes a span actually
    // selects against the token they must select.
    let sliced: Vec<&str> = spans
        .iter()
        .filter_map(|(range, _)| updated.get(range.clone()))
        .collect();
    assert!(
        sliced.contains(&"fn"),
        "no span selects the `fn` keyword exactly; spans selected {sliced:?} \
         — they were rebased onto a pre-edit parse of the fence"
    );
}
