//! Frontmatter is a code region like any other, stated as a specification:
//! the YAML between a `.md` document's `---` delimiters colours through the
//! very same tree-sitter pipeline a fenced block uses, and nothing outside
//! the delimiters is touched by it.
//!
//! `highlight::schedule_highlight` and the region resolution behind it are
//! private to `rune-tui`, so these drive the real public chokepoints
//! (`app::update`, `Cmd::run`, `highlight::visible_spans`) instead of
//! calling them directly. `clippy::panic` joins the allow list for the
//! "wrong Msg variant landed" assertions.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod highlight_common;

use highlight_common::{all_spans, app_for, type_one_char_at_end};
use rune_core::cursor::CursorSet;
use rune_syntax::ScopeId;
use rune_tui::app::{self, App};
use rune_tui::runtime::{Effects, Msg};

const FRONTMATTER: &str = "---\ntitle: \"Hello\"\ndraft: true\n---\n\n# Heading\n";
const FRONTMATTER_AND_FENCE: &str = "---\na: 1\n---\n\n```rust\nfn f() {}\n```\n";

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

/// Every span a settled document would paint, as the exact source text it
/// selects paired with the scope it carries. Comparing TEXT rather than
/// offsets is what lets a claim about a token survive the document moving
/// underneath it.
fn span_texts(app: &App) -> Vec<(String, ScopeId)> {
    let content = app.active_doc().buffer.content().to_string();
    all_spans(app)
        .into_iter()
        .filter_map(|(range, scope)| content.get(range).map(|text| (text.to_string(), scope)))
        .collect()
}

fn scope(name: &str) -> ScopeId {
    rune_syntax::scope::scope_table()
        .resolve(name)
        .expect("known scope")
}

/// The feature itself: frontmatter reaches the yaml grammar, so a key, a
/// quoted string and a boolean each land on their own scope instead of the
/// single flat run of dim grey the whole block used to be.
#[test]
fn frontmatter_yaml_is_highlighted() {
    let mut app = app_for(FRONTMATTER, "/x/notes.md");
    app.sync_view();
    settle_highlight(&mut app);

    let region = app
        .active_doc()
        .highlight
        .regions
        .first()
        .expect("the frontmatter must become a region");
    assert!(
        region.tree.is_some(),
        "the yaml grammar must have parsed the frontmatter body"
    );

    let painted = span_texts(&app);
    let selected: Vec<&str> = painted.iter().map(|(text, _)| text.as_str()).collect();
    // The quoted string's span text includes both quote characters; a plain
    // mapping key's excludes the colon. Both are the grammar's own node
    // boundaries, not this test's choice.
    for (text, name) in [
        ("title", "property"),
        ("\"Hello\"", "string"),
        ("true", "boolean"),
    ] {
        let want = scope(name);
        assert!(
            painted
                .iter()
                .any(|(got, got_scope)| got == text && *got_scope == want),
            "no span selects {text:?} at scope {name}; the pass selected {selected:?}"
        );
    }
}

/// The region covers the body only. A delimiter is not YAML, and the prose
/// after the frontmatter is not code — colouring either would mean the
/// overlay had escaped the block the document actually declared.
#[test]
fn frontmatter_spans_stay_inside_the_frontmatter() {
    let mut app = app_for(FRONTMATTER, "/x/notes.md");
    app.sync_view();
    settle_highlight(&mut app);

    let updated = app.active_doc().buffer.content().to_string();
    let body = updated
        .find("title")
        .expect("fixture has a frontmatter body");
    let close = updated
        .find("---\n\n# Heading")
        .expect("fixture has a closing delimiter");

    let spans = all_spans(&app);
    assert!(
        !spans.is_empty(),
        "the frontmatter must produce spans at all"
    );
    for (range, _) in &spans {
        assert!(
            range.start >= body && range.end <= close,
            "span {range:?} escapes the frontmatter body bytes {body}..{close} — \
             the overlay would colour text the document never declared as code"
        );
    }
}

/// Frontmatter and a fence are two independent regions in one document, each
/// with its own language. Neither may starve the other of budget, and
/// neither may be handed the other's grammar.
#[test]
fn a_document_with_frontmatter_and_a_fence_highlights_both() {
    let mut app = app_for(FRONTMATTER_AND_FENCE, "/x/notes.md");
    app.sync_view();
    settle_highlight(&mut app);

    let regions = &app.active_doc().highlight.regions;
    assert_eq!(
        regions.len(),
        2,
        "the frontmatter and the fence must each become a region"
    );
    for (i, region) in regions.iter().enumerate() {
        assert!(
            region.tree.is_some(),
            "region {i} has no retained tree — it was starved of budget"
        );
    }

    let selected: Vec<String> = span_texts(&app).into_iter().map(|(text, _)| text).collect();
    assert!(
        selected.iter().any(|text| text == "a"),
        "no span selects the yaml key `a`; the pass selected {selected:?}"
    );
    assert!(
        selected.iter().any(|text| text == "fn"),
        "no span selects the rust keyword `fn`; the pass selected {selected:?}"
    );
}

/// One keystroke can delete a region from the middle of the list: a space
/// typed into the closing `---` stops it being a delimiter, the frontmatter
/// stops existing, and every later region shifts down one index. The
/// survivor must be coloured by its OWN parse — a pass that reused the slot
/// index alone would paint the dead frontmatter's yaml onto the fence.
#[test]
fn destroying_the_frontmatter_delimiter_recolours_every_region() {
    let mut app = app_for(FRONTMATTER_AND_FENCE, "/x/notes.md");
    app.sync_view();
    settle_highlight(&mut app);
    assert_eq!(
        app.active_doc().highlight.regions.len(),
        2,
        "the fixture must start with both regions"
    );

    let close = FRONTMATTER_AND_FENCE
        .find("---\n\n```")
        .expect("fixture has a closing delimiter")
        + "---".len();
    settle_after_insert(&mut app, close, " ");

    let regions = &app.active_doc().highlight.regions;
    assert_eq!(
        regions.len(),
        1,
        "`--- ` closes nothing, so only the fence is left to be a region"
    );
    assert!(
        regions[0].tree.is_some(),
        "the surviving region must carry a parse of its own"
    );

    let updated = app.active_doc().buffer.content().to_string();
    let fence_body = updated.find("fn f()").expect("the fence body survives");
    let painted = span_texts(&app);
    let selected: Vec<&str> = painted.iter().map(|(text, _)| text.as_str()).collect();
    assert!(
        selected.contains(&"fn"),
        "no span selects the rust keyword `fn` after the shift; \
         the pass selected {selected:?}"
    );
    let property = scope("property");
    assert!(
        !painted.iter().any(|(_, got)| *got == property),
        "a yaml mapping key is still coloured after the frontmatter that \
         produced it stopped existing; the pass selected {selected:?}"
    );
    for (range, _) in all_spans(&app) {
        assert!(
            range.start >= fence_body,
            "span {range:?} lands before the fence body at {fence_body} — \
             the shifted region kept the dead frontmatter's colours"
        );
    }
}
