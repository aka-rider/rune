//! Shared fixtures for the `highlight_*` sibling test files: build a
//! `Session` around a fresh buffer, schedule a highlight by editing through
//! the real `app::update` chokepoint, settle the reply, and read the result
//! back through the same query the renderer runs. Nothing here reaches for
//! the private `highlight::schedule_highlight` — driving the public message
//! path is what makes these tests specifications of the pipeline rather than
//! of its internals. `#![allow(dead_code)]` because each consumer binary only
//! calls a subset of these — the rest would otherwise trip `-D warnings`'
//! dead-code lint in that particular binary.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ops::Range;

use rune_fuzz::Session;
use rune_syntax::ScopeId;
use rune_tui::app::{self, App};
use rune_tui::highlight::{self, HighlightReply, RegionOutcome, RegionPayload, RegionResult};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::linemap::LineMap;
use rune_tui::runtime::{Effects, Msg};

const RIGHT: KeyInput = KeyInput {
    code: KeyCode::Right,
    mods: Mods::NONE,
};

const LEFT: KeyInput = KeyInput {
    code: KeyCode::Left,
    mods: Mods::NONE,
};

pub fn app_for(content: &str, path: &str) -> Session {
    Session::open(path, content)
}

/// Walks the active document's caret from wherever it sits to `offset`, one
/// real `Left`/`Right` press per grapheme step — never a `CursorSet::new`
/// poke. Bidirectional: the caret can already sit past `offset` (a prior
/// settle left it at the document's end). `KeyCode::End` is deliberately NOT
/// used for the forward case: it moves to the end of the CURRENT line
/// (`Motion::LineEnd`), not the end of the document, and a document-end walk
/// needs to cross line boundaries too.
fn place_caret(session: &mut Session, offset: usize) {
    let len = session.app().active_doc().buffer.content().len();
    let target = offset.min(len);
    let mut guard = 0usize;
    loop {
        let position = session.app().active_doc().cursors.primary().position;
        if position == target {
            break;
        }
        session.key(if position < target { RIGHT } else { LEFT });
        guard += 1;
        assert!(
            guard <= len + 8,
            "caret placement stalled before reaching offset {target}"
        );
    }
}

/// Types one harmless character at the END of the active document's buffer
/// through the real `app::update` chokepoint, bumping its
/// buffer version so `App::update`'s own before/after gate schedules a
/// highlight `Cmd` — mirrors how a real keystroke does it. Scheduling
/// refreshes the block tree itself, so an edit BEFORE a fence is equally
/// safe — the sibling regression test below covers exactly that.
pub fn type_one_char_at_end(session: &mut Session, effects: &mut Effects) {
    let end = session.app().active_doc().buffer.content().len();
    place_caret(session, end);
    app::update(
        session.app_mut(),
        Msg::Key(KeyInput {
            code: KeyCode::Char('!'),
            mods: Mods::NONE,
        }),
        effects,
    );
}

/// Runs the document's pending highlight to completion through the real
/// message path: schedule (by typing one character), run the `Cmd` inline,
/// deliver its reply. The state it leaves behind is read back through
/// `all_spans`, the same query the renderer uses.
pub fn settle_highlight(session: &mut Session) {
    let mut effects = Effects::default();
    type_one_char_at_end(session, &mut effects);
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
    app::update(session.app_mut(), msg, &mut effects);
}

/// Schedules a highlight by inserting `text` at `at` — a version bump the
/// caller controls, unlike the append `type_one_char_at_end` performs — and
/// settles the reply. Walks the caret to `at` with real `Right` presses from
/// byte 0 (`Session::open`'s own starting caret), never a `CursorSet::new`
/// poke.
pub fn settle_after_insert(session: &mut Session, at: usize, text: &str) {
    place_caret(session, at);
    let mut effects = Effects::default();
    app::update(
        session.app_mut(),
        Msg::Paste(text.to_string()),
        &mut effects,
    );
    for cmd in effects.cmds.drain(..) {
        if let Some(msg) = cmd.run() {
            let mut settled = Effects::default();
            app::update(session.app_mut(), msg, &mut settled);
        }
    }
}

/// A `Msg::Highlighted` payload carrying one span-backed region — the shape
/// a ```` ```markdown ```` fence produces, and the only shape a test can
/// hand-build (a `ParsedTree` cannot be synthesized). `LineMap::default()`
/// maps nothing, which is correct: a span-backed region's spans are already
/// buffer offsets.
pub fn span_reply(spans: Vec<(Range<usize>, ScopeId)>) -> HighlightReply {
    HighlightReply {
        regions: vec![RegionResult {
            map: LineMap::default(),
            outcome: RegionOutcome::Replace(RegionPayload::Spans {
                source: String::new(),
                spans,
            }),
        }],
        truncated: false,
    }
}

/// Every span the active document would paint anywhere — the same query the
/// renderer runs, over the whole buffer instead of one viewport window.
pub fn all_spans(app: &App) -> Vec<(Range<usize>, ScopeId)> {
    let doc = app.active_doc();
    highlight::visible_spans(doc, 0..doc.buffer.content().len())
}

/// Whether the active document's region at `index` is backed by a retained
/// tree — the state a reuse test has to observe to mean anything.
pub fn region_tree_source(app: &App, index: usize) -> Option<String> {
    app.active_doc()
        .highlight
        .regions
        .get(index)
        .and_then(|region| region.tree.as_ref())
        .map(|tree| tree.source().to_string())
}
