//! Repro for #37: the session fuzzer's `Action::Highlight` only ever
//! delivers `Msg::Highlighted` through the `RegionPayload::Spans` channel
//! with `LineMap::default()`, so the `RegionPayload::Tree` channel —
//! `install_regions`, queried per-frame by `visible_spans` through
//! `LineMap::reconstructed_window` — has never been exercised by fuzzing.
//!
//! These tests pin the naive-port trap a real-parse fuzz action must avoid:
//! a `Tree` reply carrying a default `LineMap` maps nothing and stays
//! invisible, while the same tree carrying the region's real physical-line
//! ranges surfaces spans. Both deliver `Msg::Highlighted` through
//! `rune_tui::app::update`, the same path the driver's own
//! `Action::Highlight` arm uses — never by poking `doc.highlight` fields
//! directly.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use common::new_app;
use rune_fuzz::action::tree_fixture_line_ranges;
use rune_tui::app::{self, App};
use rune_tui::highlight::{
    HighlightReply, PassOutcome, RegionOutcome, RegionPayload, RegionResult, visible_spans,
};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::linemap::LineMap;
use rune_tui::runtime::{Effects, Msg};

const FIXTURE: &str = "{\n  \"a\": 1,\n  \"b\": [true, null]\n}";

fn deliver_tree_reply(app: &mut App, map: LineMap, tree: rune_ts::ParsedTree, version: u64) {
    let msg = Msg::Highlighted {
        doc: app.active,
        version,
        result: PassOutcome::Replace(HighlightReply {
            regions: vec![RegionResult {
                map,
                outcome: RegionOutcome::Replace(RegionPayload::Tree(tree)),
            }],
            truncated: false,
        }),
    };
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
}

fn parse_fixture() -> rune_ts::ParsedTree {
    rune_ts::parse("json", FIXTURE, Duration::from_secs(5))
        .expect("the JSON fixture must parse for this repro to mean anything")
}

#[test]
fn tree_reply_with_default_linemap_is_invisible() {
    let mut app = new_app(FIXTURE);
    let live_version = app.active_doc().buffer.version();
    deliver_tree_reply(&mut app, LineMap::default(), parse_fixture(), live_version);

    let doc = app.active_doc();
    let spans = visible_spans(doc, 0..doc.buffer.content().len());
    assert!(
        spans.is_empty(),
        "a Tree reply delivered with a default LineMap must stay invisible, got {spans:?}"
    );
}

#[test]
fn tree_reply_with_real_linemap_surfaces_spans() {
    let mut app = new_app(FIXTURE);
    let live_version = app.active_doc().buffer.version();
    let map = LineMap::new(FIXTURE, tree_fixture_line_ranges(FIXTURE, 0));
    deliver_tree_reply(&mut app, map, parse_fixture(), live_version);

    let doc = app.active_doc();
    let content = doc.buffer.content();
    let spans = visible_spans(doc, 0..content.len());
    assert!(
        !spans.is_empty(),
        "a Tree reply delivered with the fixture's real LineMap must surface spans"
    );
    for (range, _) in &spans {
        assert!(range.start < range.end, "inverted or empty span {range:?}");
        assert!(
            range.end <= content.len(),
            "span past content.len() {range:?}"
        );
        assert!(
            content.is_char_boundary(range.start),
            "start off a char boundary {range:?}"
        );
        assert!(
            content.is_char_boundary(range.end),
            "end off a char boundary {range:?}"
        );
    }
}

#[test]
fn stale_tree_reply_is_dropped() {
    let mut app = new_app(FIXTURE);

    // Bump the buffer version past 0 first, so "live - 1" names a genuinely
    // earlier version rather than underflowing.
    let key = KeyInput {
        code: KeyCode::Char(' '),
        mods: Mods::NONE,
    };
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Key(key), &mut effects);

    let content = app.active_doc().buffer.content().to_string();
    let before = visible_spans(app.active_doc(), 0..content.len());

    let live_version = app.active_doc().buffer.version();
    assert!(
        live_version >= 1,
        "the edit above must have bumped the version"
    );
    let map = LineMap::new(FIXTURE, tree_fixture_line_ranges(FIXTURE, 0));
    deliver_tree_reply(&mut app, map, parse_fixture(), live_version - 1);

    let after = visible_spans(app.active_doc(), 0..content.len());
    assert_eq!(
        before, after,
        "a stale-version Tree reply must leave the region's spans untouched"
    );
}
