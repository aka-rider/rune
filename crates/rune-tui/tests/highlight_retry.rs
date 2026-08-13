//! `Msg::Highlighted` reply semantics — keep-on-`None` (`[R2]`),
//! drop-on-stale-version, the single-bounded-parse timeout, and the status
//! line a reply surfaces (or must stay silent about).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod highlight_common;

use highlight_common::{all_spans, app_for, span_reply};
use rune_fuzz::Session;
use rune_syntax::scope::scope_table;
use rune_tui::app;
use rune_tui::highlight::{HighlightReply, RegionPayload, RegionResult};
use rune_tui::linemap::LineMap;
use rune_tui::runtime::{CmdKind, Effects, Msg};

/// A timed-out reply posts a message, and an open message pane arms its own
/// auto-collapse timer — so "no retry was scheduled" is a claim about the
/// highlight chain specifically, never about the effect list being empty.
fn schedules_a_highlight_cmd(effects: &Effects) -> bool {
    effects
        .cmds
        .iter()
        .any(|cmd| cmd.kind() == CmdKind::Highlight)
}

/// Installs one span-backed region carrying `spans` through the real
/// `app::update` chokepoint, at the live buffer version.
fn install(session: &mut Session, spans: Vec<(std::ops::Range<usize>, rune_syntax::ScopeId)>) {
    let app = session.app_mut();
    let id = app.active;
    let version = app.doc(id).expect("doc").buffer.version();
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Highlighted {
            doc: id,
            version,
            result: Some(span_reply(spans)),
        },
        &mut effects,
    );
}

#[test]
fn none_result_leaves_spans_byte_identical() {
    let mut session = app_for("fn main() {}\n", "/x/main.rs");
    let id = session.app().active;
    let keyword = scope_table().resolve("keyword").expect("known scope");
    install(&mut session, vec![(0..2, keyword)]);
    let before = all_spans(session.app());
    assert_eq!(before, vec![(0..2, keyword)]);
    let version = session.app().doc(id).expect("doc").buffer.version();

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Highlighted {
            doc: id,
            version,
            result: None,
        },
        &mut effects,
    );

    assert_eq!(all_spans(session.app()), before);
}

#[test]
fn reply_at_a_stale_version_leaves_spans_unchanged() {
    let mut session = app_for("fn main() {}\n", "/x/main.rs");
    let id = session.app().active;
    let keyword = scope_table().resolve("keyword").expect("known scope");
    install(&mut session, vec![(0..2, keyword)]);
    let before = all_spans(session.app());
    let stale_version = session.app().doc(id).expect("doc").buffer.version();

    // Advance the buffer past `stale_version` without going through a real
    // edit command — a direct field write is the same convention
    // `tests/tui_render.rs::app_for` already used for other `Document`
    // fields (`cursors`, `viewport`) before the migration onto `Session`.
    {
        let app = session.app_mut();
        let doc = app.doc_mut(id).expect("doc");
        doc.buffer = doc
            .buffer
            .insert(0, "x")
            .expect("in-bounds insert should apply");
    }

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Highlighted {
            doc: id,
            version: stale_version,
            result: Some(span_reply(vec![(0..3, keyword)])),
        },
        &mut effects,
    );

    assert_eq!(all_spans(session.app()), before);
}

/// A `None` reply for a never-yet-highlighted document (every region's parse
/// timed out, hit an unresolvable language, or failed) surfaces a status
/// line — in ONE attempt, since there is a single bounded parse per region
/// and no retry chain.
#[test]
fn a_timed_out_document_surfaces_a_message() {
    let content = "fn main() {}\n";
    let mut session = app_for(content, "/x/main.rs");
    let id = session.app().active;
    let version = session.app().doc(id).expect("doc").buffer.version();
    assert_eq!(
        session.app().doc(id).expect("doc").highlight.version,
        0,
        "a fresh document must never have been highlighted yet"
    );

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Highlighted {
            doc: id,
            version,
            result: None,
        },
        &mut effects,
    );

    let doc = session.app().doc(id).expect("doc");
    assert!(
        doc.highlight.regions.is_empty(),
        "a None reply must never invent a region"
    );
    assert_eq!(
        doc.highlight.in_flight, None,
        "in_flight must still clear on a timed-out reply, or this document \
        could never be highlighted again by any future edit"
    );
    assert!(
        !schedules_a_highlight_cmd(&effects),
        "a single-attempt timeout schedules no further highlight cmd — there \
        is no retry chain to continue"
    );
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("syntax highlighting timed out for this document"),
        "a timed-out document's parse must surface a status line, not fail \
        silently"
    );
}

/// The unification this refactor exists for, on the timeout path: a markdown
/// document whose fence timed out reports exactly like a file that timed
/// out. The two used to differ — a fence's timeout was silent, because the
/// status branch was gated on the document having a whole-buffer language.
#[test]
fn a_timed_out_markdown_fence_surfaces_the_same_message() {
    let content = "```rust\nfn main() {}\n```\n";
    let mut session = app_for(content, "/x/notes.md");
    let id = session.app().active;
    let version = session.app().doc(id).expect("doc").buffer.version();

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Highlighted {
            doc: id,
            version,
            result: None,
        },
        &mut effects,
    );

    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("syntax highlighting timed out for this document"),
        "a fence that times out must report like a file that times out"
    );
}

/// A document that has already been highlighted once must never re-surface
/// the timeout status on a later reparse-after-edit that overruns the
/// budget: its existing colours are still good, so the reply degrades to
/// STALE colours per `[R2]` and stays silent rather than spamming the status
/// on every settled edit of a large file.
#[test]
fn a_reparse_timeout_on_an_already_highlighted_document_stays_silent() {
    let content = "fn main() {}\n";
    let mut session = app_for(content, "/x/main.rs");
    let id = session.app().active;
    let keyword = scope_table().resolve("keyword").expect("known scope");
    install(&mut session, vec![(0..2, keyword)]);

    let version = session.app().doc(id).expect("doc").buffer.version();
    assert_eq!(
        session.app().doc(id).expect("doc").highlight.version,
        version,
        "the first successful reply must stamp highlight.version"
    );
    let spans_before = all_spans(session.app());

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Highlighted {
            doc: id,
            version,
            result: None,
        },
        &mut effects,
    );

    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        None,
        "a reparse timeout on an already-highlighted document must stay \
        silent, not surface the timed-out status a second time"
    );
    assert_eq!(
        all_spans(session.app()),
        spans_before,
        "the stale-but-good spans from the first successful reply must be \
        left untouched"
    );
}

/// A terminal timeout must never re-dispatch a further parse when `pending`
/// was armed only by a document switch (no edit): an edit-armed `pending`
/// carries a different version and lands in the stale arm instead, so
/// `pending` and a live-version `None` can coincide only in this no-edit
/// case, where re-scheduling would just repeat the same doomed parse.
#[test]
fn a_timeout_with_pending_armed_schedules_no_further_cmd() {
    let content = "fn main() {}\n";
    let mut session = app_for(content, "/x/main.rs");
    let id = session.app().active;
    let version = session.app().doc(id).expect("doc").buffer.version();
    session
        .app_mut()
        .doc_mut(id)
        .expect("doc")
        .highlight
        .pending = true;

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Highlighted {
            doc: id,
            version,
            result: None,
        },
        &mut effects,
    );

    assert!(
        !schedules_a_highlight_cmd(&effects),
        "a terminal timeout must schedule no further highlight cmd even with \
        `pending` armed by a mere document switch"
    );
    assert_eq!(
        session.app().doc(id).expect("doc").highlight.in_flight,
        None,
        "the timed-out reply must still clear in_flight"
    );
}

/// A reply whose payload is `None` for a region keeps whatever that region
/// already held while still taking its refreshed map — the mechanism behind
/// both "my tree is still valid" and "my reparse overran the budget".
#[test]
fn a_payload_less_region_slot_carries_its_existing_colours_forward() {
    let content = "fn main() {}\n";
    let mut session = app_for(content, "/x/main.rs");
    let id = session.app().active;
    let keyword = scope_table().resolve("keyword").expect("known scope");
    install(&mut session, vec![(0..2, keyword)]);
    let before = all_spans(session.app());

    let version = session.app().doc(id).expect("doc").buffer.version();
    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Highlighted {
            doc: id,
            version,
            result: Some(HighlightReply {
                regions: vec![RegionResult {
                    map: LineMap::default(),
                    payload: None,
                }],
                truncated: false,
            }),
        },
        &mut effects,
    );

    assert_eq!(all_spans(session.app()), before);
}

/// A reply carrying `truncated: true` (a producer hit its span cap) must
/// surface a status line telling the user part of the document is
/// uncoloured, just as a timed-out reply already does for the "nothing
/// coloured at all" case.
#[test]
fn span_cap_truncation_surfaces_a_status_line() {
    let content = "fn main() {}\n";
    let mut session = app_for(content, "/x/main.rs");
    let id = session.app().active;
    let version = session.app().doc(id).expect("doc").buffer.version();
    let keyword = scope_table().resolve("keyword").expect("known scope");

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Highlighted {
            doc: id,
            version,
            result: Some(HighlightReply {
                regions: vec![RegionResult {
                    map: LineMap::default(),
                    payload: Some(RegionPayload::Spans(vec![(0..2, keyword)])),
                }],
                truncated: true,
            }),
        },
        &mut effects,
    );

    let doc = session.app().doc(id).expect("doc");
    assert!(doc.highlight.truncated, "the truncated flag must be stored");
    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("syntax highlighting was truncated; the tail of this document is uncoloured"),
        "a truncated reply must surface a status line, not fail silently"
    );
}

/// When a reply's `truncated` state (sticky on `Document::highlight` from an
/// earlier reply) and this reply's own `timed_out` outcome both hold,
/// exactly one status line is shown, and it is the timeout message — a
/// timed-out reply means nothing was coloured this round at all, which is
/// more actionable than "coloured, but part is missing".
#[test]
fn timeout_outranks_truncation_in_the_status_line() {
    let content = "fn main() {}\n";
    let mut session = app_for(content, "/x/main.rs");
    let id = session.app().active;
    let version = session.app().doc(id).expect("doc").buffer.version();
    session
        .app_mut()
        .doc_mut(id)
        .expect("doc")
        .highlight
        .truncated = true;

    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Highlighted {
            doc: id,
            version,
            result: None,
        },
        &mut effects,
    );

    assert_eq!(
        rune_tui::messages::newest_text(session.app()),
        Some("syntax highlighting timed out for this document"),
        "the timeout message must win over a sticky truncated flag"
    );
}
