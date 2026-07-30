//! Split off `highlight.rs` (WP11, §1.6): `Msg::Highlighted` reply
//! semantics — keep-on-`None` (`[R2]`), drop-on-stale-version, the D5/D6
//! single-bounded-parse timeout/tree-payload handling, and the status line
//! a reply surfaces (or must stay silent about).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod highlight_common;

use std::time::Duration;

use highlight_common::app_for;
use rune_syntax::scope::scope_table;
use rune_tui::app;
use rune_tui::runtime::{Effects, HighlightPayload, HighlightResult, Msg};

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
        doc.buffer = doc
            .buffer
            .insert(0, "x")
            .expect("in-bounds insert should apply");
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

/// A document that has already been highlighted once must never re-surface
/// the timeout status on a later reparse-after-edit that overruns the
/// budget: its existing spans/tree are still good, so the reply degrades to
/// STALE colours per `[R2]` and stays silent rather than spamming the
/// status on every settled edit of a large file.
#[test]
fn a_reparse_timeout_on_an_already_highlighted_document_stays_silent() {
    let content = "fn main() {}\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;
    let version = app.doc(id).expect("doc").buffer.version();

    let keyword = scope_table().resolve("keyword").expect("known scope");
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version,
            result: Some(HighlightPayload::Spans(vec![(0..2, keyword)].into())),
        },
        &mut effects,
    );

    let doc = app.doc(id).expect("doc");
    assert_eq!(
        doc.highlight.version, version,
        "the first successful reply must stamp highlight.version"
    );
    let spans_before = doc.highlight.spans.clone();

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
    assert_eq!(
        app.status_message, None,
        "a reparse timeout on an already-highlighted document must stay \
         silent, not surface the timed-out status a second time"
    );
    assert_eq!(
        doc.highlight.spans, spans_before,
        "the stale-but-good spans from the first successful reply must be \
         left untouched"
    );
}

/// A terminal timeout must never re-dispatch a further parse when `pending`
/// was armed only by a document switch (no edit): an edit-armed `pending`
/// carries a different version and lands in the stale `_` arm instead, so
/// `pending` and a live-version `None` can coincide only in this no-edit
/// case, where re-scheduling would just repeat the same doomed parse.
#[test]
fn a_timeout_with_pending_armed_schedules_no_further_cmd() {
    let content = "fn main() {}\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;
    let version = app.doc(id).expect("doc").buffer.version();
    app.doc_mut(id).expect("doc").highlight.pending = true;

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

    assert!(
        effects.cmds.is_empty(),
        "a terminal timeout must schedule no further cmd even with \
         `pending` armed by a mere document switch"
    );
    assert_eq!(
        app.doc(id).expect("doc").highlight.in_flight,
        None,
        "the timed-out reply must still clear in_flight"
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

/// The sibling of `a_timed_out_code_document_surfaces_a_status_message`:
/// a `Spans` reply whose payload carries `truncated: true` (the producer
/// hit its span cap) must surface a status line telling the user the tail
/// of the document is uncoloured, just as a timed-out reply already does
/// for the "nothing coloured at all" case.
#[test]
fn span_cap_truncation_surfaces_a_status_line() {
    let content = "fn main() {}\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;
    let version = app.doc(id).expect("doc").buffer.version();
    let keyword = scope_table().resolve("keyword").expect("known scope");

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version,
            result: Some(HighlightPayload::Spans(HighlightResult {
                spans: vec![(0..2, keyword)],
                truncated: true,
            })),
        },
        &mut effects,
    );

    let doc = app.doc(id).expect("doc");
    assert!(doc.highlight.truncated, "the truncated flag must be stored");
    assert_eq!(
        app.status_message.as_deref(),
        Some("syntax highlighting was truncated; the tail of this document is uncoloured"),
        "a truncated reply must surface a status line, not fail silently"
    );
}

/// Pins the WP4.S2 decision: when a reply's `truncated` state (sticky on
/// `Document::highlight` from an earlier `Spans` reply) and this reply's
/// own `timed_out` outcome (freshly decided on a `None` result) both hold,
/// exactly one status line is shown, and it is the timeout message — a
/// timed-out reply means nothing was coloured this round at all, which is
/// more actionable than "coloured, but the tail is missing".
#[test]
fn timeout_outranks_truncation_in_the_status_line() {
    let content = "fn main() {}\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;
    let version = app.doc(id).expect("doc").buffer.version();
    app.doc_mut(id).expect("doc").highlight.truncated = true;

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
        app.status_message.as_deref(),
        Some("syntax highlighting timed out for this document"),
        "the timeout message must win over a sticky truncated flag"
    );
}
