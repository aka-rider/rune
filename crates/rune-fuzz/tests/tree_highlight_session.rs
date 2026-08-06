//! Driver-level acceptance test for issue #37: `Action::HighlightTree`
//! actually reaches a REAL user through the render-time query, not just
//! `install_regions` in isolation (`tests/tree_highlight_reply.rs`'s own
//! pinning repro).
//!
//! Two things are proven here, split across two kinds of test because
//! `driver::RunResult` only ever carries a `Snapshot` back out when a
//! session hits an invariant violation (`final_snapshot` is `None` on a
//! clean run — `final_content` is the only field a passing session leaves
//! populated): a scripted session, driven end to end through the real
//! `rune_tui::app::update` via `driver::run`, must settle with no invariant
//! violation (`HL-CLAMPED`/`HL-STALE-DROP`/`HL-NO-REFLOW` all key off
//! `MsgTag::Highlighted`, exactly the tag `Action::HighlightTree` carries);
//! and, delivering the identical `Msg::Highlighted` construction the
//! driver's own `Action::HighlightTree` arm builds — `LineMap::new`d from
//! `tree_fixture_line_ranges`, payload from a real `rune_ts::parse` —
//! directly against an `App` must surface non-empty `visible_spans` for a
//! LIVE version and leave them unchanged for a STALE one, proving the spans
//! are sourced from the TREE arm of `install_regions`: a `Tree` reply
//! carries no `RegionPayload::Spans` at all.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use rune_core::buffer::Buffer;
use rune_fuzz::action::{Action, HighlightVersion, tree_fixture, tree_fixture_line_ranges};
use rune_fuzz::driver;
use rune_tui::app::{self, App};
use rune_tui::highlight::{HighlightReply, RegionPayload, RegionResult, visible_spans};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::linemap::LineMap;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::{Mem, Vfs};

/// The multi-line entry in `TREE_FIXTURES` — spans more than one physical
/// line, so its `LineMap` has more than one range to reconstruct.
const FIXTURE_INDEX: u8 = 1;

/// A single-byte-per-char filler long enough that every span the fixture's
/// tree can produce survives the render query's `end <= content.len()`
/// clamp once typed into the buffer.
fn filler_at_least(len: usize) -> String {
    "a".repeat(len)
}

fn escape_then_h_actions(filler: &str) -> Vec<Action> {
    vec![
        Action::Key(KeyInput {
            code: KeyCode::Escape,
            mods: Mods::NONE,
        }),
        Action::Key(KeyInput {
            code: KeyCode::Char('h'),
            mods: Mods::NONE,
        }),
        Action::Type(filler.to_string()),
    ]
}

fn assert_clean_session(version: HighlightVersion) {
    let filler = filler_at_least(tree_fixture(FIXTURE_INDEX).len());
    let mut actions = escape_then_h_actions(&filler);
    actions.push(Action::HighlightTree {
        version,
        fixture: FIXTURE_INDEX,
        base: 0,
    });

    let result = driver::run(driver::DOC_PATH, "", &actions);
    assert!(
        result.violation.is_none(),
        "session must settle with no invariant violation for {version:?}, got {:?}",
        result.violation.as_ref().map(|v| (v.id, &v.message))
    );
}

#[test]
fn live_highlight_tree_session_has_no_invariant_violation() {
    assert_clean_session(HighlightVersion::Live);
}

#[test]
fn stale_highlight_tree_session_has_no_invariant_violation() {
    assert_clean_session(HighlightVersion::Stale);
}

fn new_app(content: &str) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let mut app = App::new(Buffer::new(content), None, vfs, None);
    app.frame_width = 80;
    app.frame_height = 24;
    app.relayout();
    app.sync_view();
    app
}

fn press_key(app: &mut App, key: KeyInput) {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(key), &mut effects);
}

fn type_text(app: &mut App, text: &str) {
    for ch in text.chars() {
        let code = if ch == '\n' {
            KeyCode::Enter
        } else {
            KeyCode::Char(ch)
        };
        press_key(
            app,
            KeyInput {
                code,
                mods: Mods::NONE,
            },
        );
    }
}

/// Exactly the message `driver`'s `Action::HighlightTree` arm constructs.
fn deliver_tree(app: &mut App, version: u64, base: usize) {
    let source = tree_fixture(FIXTURE_INDEX);
    let map = LineMap::new(tree_fixture_line_ranges(source, base));
    let payload = rune_ts::parse("json", source, Duration::from_secs(5)).map(RegionPayload::Tree);
    let msg = Msg::Highlighted {
        doc: app.active,
        version,
        result: Some(HighlightReply {
            regions: vec![RegionResult { map, payload }],
            truncated: false,
        }),
    };
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
}

/// Seeds an `App` whose buffer starts with the fixture's own byte length in
/// filler content — so a `base: 0` tree reply's `LineMap` lands on exactly
/// those bytes — then appends one more character AFTER the cursor moves to
/// the end, bumping the buffer version without disturbing byte `0..fixture
/// len()`.
fn seeded_app_with_bumped_version(fixture_len: usize) -> App {
    let mut app = new_app(&filler_at_least(fixture_len));
    press_key(
        &mut app,
        KeyInput {
            code: KeyCode::End,
            mods: Mods::NONE,
        },
    );
    type_text(&mut app, "z");
    app
}

#[test]
fn live_tree_reply_surfaces_nonempty_spans() {
    let fixture_len = tree_fixture(FIXTURE_INDEX).len();
    let mut app = seeded_app_with_bumped_version(fixture_len);

    let live_version = app.active_doc().buffer.version();
    assert!(live_version >= 1, "the edit above must have bumped the version");
    deliver_tree(&mut app, live_version, 0);

    let doc = app.active_doc();
    let spans = visible_spans(doc, 0..doc.buffer.content().len());
    assert!(
        !spans.is_empty(),
        "a live-version Tree reply must surface non-empty highlight spans, got {spans:?}"
    );
}

#[test]
fn stale_tree_reply_leaves_spans_unchanged() {
    let fixture_len = tree_fixture(FIXTURE_INDEX).len();
    let mut app = seeded_app_with_bumped_version(fixture_len);

    let live_version = app.active_doc().buffer.version();
    assert!(live_version >= 1, "the edit above must have bumped the version");

    let content = app.active_doc().buffer.content().to_string();
    let before = visible_spans(app.active_doc(), 0..content.len());

    deliver_tree(&mut app, live_version - 1, 0);

    let after = visible_spans(app.active_doc(), 0..content.len());
    assert_eq!(
        before, after,
        "a stale-version Tree reply must leave the region's spans untouched"
    );
    assert!(
        after.is_empty(),
        "no earlier reply was ever delivered in this session, so spans must still be empty, \
         got {after:?}"
    );
}
