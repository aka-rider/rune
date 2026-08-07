//! Shared fixtures for the `highlight_*` sibling test files: build an `App`
//! around a fresh buffer, schedule a highlight by editing through the real
//! `app::update` chokepoint, settle the reply, and read the result back
//! through the same query the renderer runs. Nothing here reaches for the
//! private `highlight::schedule_highlight` — driving the public message path
//! is what makes these tests specifications of the pipeline rather than of
//! its internals. `#![allow(dead_code)]` because each consumer binary only
//! calls a subset of these — the rest would otherwise trip `-D warnings`'
//! dead-code lint in that particular binary.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_syntax::ScopeId;
use rune_tui::app::{self, App};
use rune_tui::highlight::{self, HighlightReply, RegionPayload, RegionResult};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::linemap::LineMap;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

pub fn app_for(content: &str, path: &str) -> App {
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
pub fn type_one_char_at_end(app: &mut App, effects: &mut Effects) {
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

/// Runs the document's pending highlight to completion through the real
/// message path: schedule (by typing one character), run the `Cmd` inline,
/// deliver its reply. The state it leaves behind is read back through
/// `all_spans`, the same query the renderer uses.
pub fn settle_highlight(app: &mut App) {
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
pub fn settle_after_insert(app: &mut App, at: usize, text: &str) {
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

/// A `Msg::Highlighted` payload carrying one span-backed region — the shape
/// a ```` ```markdown ```` fence produces, and the only shape a test can
/// hand-build (a `ParsedTree` cannot be synthesized). `LineMap::default()`
/// maps nothing, which is correct: a span-backed region's spans are already
/// buffer offsets.
pub fn span_reply(spans: Vec<(Range<usize>, ScopeId)>) -> HighlightReply {
    HighlightReply {
        regions: vec![RegionResult {
            map: LineMap::default(),
            payload: Some(RegionPayload::Spans(spans)),
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
