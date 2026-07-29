//! The root machine: owns focus state and the wrap width every downstream
//! element inherits (plan Context, "Root machine"). `DocMachine::sync_cursors`
//! is the ONLY place child `RevealSm::transition` calls fire; `sync_content`
//! reparses iff the buffer version changed, and reveal transitions never
//! touch `built_version` (Gotchas: "Reveal must never bump the buffer
//! version").

use std::ops::Range;

use crate::element::block::Block;
use crate::snapshot::DisplaySnapshot;
use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_syntax::SyntaxSnapshot;
use rune_syntax::element::{CursorProbe, DocState, InheritCtx, RevealGrant, WrapState};
use rune_syntax::kind::DocumentKind;
use rune_syntax::wrap::WrapSnapshot;

/// `emit` -> wrap (root-owned, keyed off `self.wrap`) -> `DisplaySnapshot`,
/// per `DocMachine::snapshot`. `syntax`/`wrap` carry the coordinate
/// conversions (`buffer_to_syntax`/`syntax_to_buffer`,
/// `syntax_to_wrap`/`wrap_to_syntax`); `display` is the wrap rows with
/// table borders synthesised in (WP3's `DisplaySnapshot::expand_tables`) —
/// every display-space consumer (rendering, the viewport, mouse
/// hit-testing) reads row geometry from `display`, never `wrap` directly.
#[derive(Clone)]
pub struct ViewSnapshots {
    pub syntax: SyntaxSnapshot,
    pub wrap: WrapSnapshot,
    pub display: DisplaySnapshot,
}

pub struct DocMachine {
    state: DocState,
    wrap: WrapState,
    blocks: Vec<Block>,
    built_version: u64,
    dirty: bool,
    kind: DocumentKind,
    /// The memo `snapshot` returns a clone of when `dirty` is false —
    /// `None` only before the first `snapshot` call. `dirty` is the single
    /// guard: every setter that can change what `snapshot` would compute
    /// (`sync_content` on a version change, `set_width`, `sync_cursors`/
    /// `set_focus` on a reveal-relevant change) sets it, so a `view()` call
    /// that changed none of those inputs gets the cached clone instead of
    /// re-running emit + wrap + `expand_tables`.
    cached: Option<ViewSnapshots>,
    /// Counts actual `rebuild` calls (never memo hits) — test-only
    /// instrumentation for asserting `snapshot`'s memoization, not a
    /// production concern.
    #[cfg(test)]
    rebuild_count: std::cell::Cell<usize>,
}

impl Default for DocMachine {
    fn default() -> Self {
        DocMachine::new()
    }
}

impl DocMachine {
    pub fn new() -> DocMachine {
        DocMachine {
            state: DocState::Unfocused,
            wrap: WrapState::default(),
            blocks: Vec::new(),
            // `Buffer::version()` starts at 1 (see rune-core), so 0 can never
            // equal a real buffer version — the first `sync_content` call
            // always reparses without a separate "never built yet" flag.
            built_version: 0,
            dirty: true,
            kind: DocumentKind::Markdown,
            cached: None,
            #[cfg(test)]
            rebuild_count: std::cell::Cell::new(0),
        }
    }

    /// Test-only: the number of `rebuild` calls (memo misses) so far.
    #[cfg(test)]
    pub(crate) fn rebuild_count(&self) -> usize {
        self.rebuild_count.get()
    }

    /// The buffer version `blocks` was last built from — `0` before the
    /// first `sync_content` call (never a real `Buffer::version()`, which
    /// starts at 1). `Document::view` reads this before/after `sync_content`
    /// to decide whether the catalogue needs rebuilding, without needing its
    /// own copy of the reparse guard.
    pub fn built_version(&self) -> u64 {
        self.built_version
    }

    /// The single writer of `state` for `DocMachine` — the crate's other
    /// transition writer (`RevealSm::transition` in `element/mod.rs` is the
    /// first; this is the second and last).
    fn transition(&mut self, next: DocState) {
        let prev = self.state;
        self.state = next;
        self.enter_state(prev);
    }

    fn enter_state(&mut self, _prev: DocState) {
        self.dirty = true;
    }

    pub fn state(&self) -> DocState {
        self.state
    }

    pub fn wrap(&self) -> WrapState {
        self.wrap
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Every fenced code block's language tag and per-line content byte
    /// ranges (plan WP6.S1) — walked recursively into blockquotes and list
    /// items so a fence nested inside either is found too, not only a
    /// top-level one. Returns `CodeFenceM::content_lines` itself, ONE
    /// `Range` per physical content line, never collapsed into a single
    /// `first.start..last.end` span: a container's own repeating prefix
    /// (`"> "`, a list marker's indent) sits in the GAP between two
    /// consecutive lines' buffer ranges, so a single contiguous range
    /// covering both would include it, while the per-line list lets the
    /// caller reconstruct a prefix-free source and map spans back through
    /// the gaps instead. A fence with an empty `language` or no content
    /// lines contributes nothing.
    pub fn code_fences(&self) -> Vec<(&str, Vec<Range<usize>>)> {
        let mut out = Vec::new();
        collect_code_fences(&self.blocks, &mut out);
        out
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    pub fn set_focus(&mut self, focused: bool) {
        let next = if focused {
            DocState::Focused
        } else {
            DocState::Unfocused
        };
        if next != self.state {
            self.transition(next);
        }
    }

    /// Wrap-width change; marks dirty but fires NO reveal transitions
    /// (Gotchas: conflating wrap/focus changes with content-version changes
    /// would re-parse on every arrow key).
    pub fn set_width(&mut self, width: u16) {
        if self.wrap.width != width {
            self.wrap.width = width;
            self.dirty = true;
        }
    }

    /// Selects which producer `sync_content` runs — comrak for `Markdown`,
    /// no parse at all (verbatim per-line text, plan WP4 decision 6) for
    /// `Code`/`Plain`. Marks dirty only when the kind actually changes, so
    /// re-binding a document to the kind it already has (e.g. re-opening
    /// the same path) doesn't force a needless reparse.
    pub fn set_kind(&mut self, kind: DocumentKind) {
        if kind != self.kind {
            self.kind = kind;
            self.dirty = true;
        }
    }

    /// Rebuild the block/inline tree iff the buffer version changed. A pure
    /// cursor move never bumps `buf.version()`, so this is a no-op on every
    /// keystroke that isn't a content edit. For a non-markdown `kind`, no
    /// comrak parse happens at all — `blocks` is emptied and `snapshot`'s
    /// `emit` call turns an empty block list into one verbatim `Identical`
    /// span per line (plan WP4 decision 6: no second plain-text producer).
    pub fn sync_content(&mut self, buf: &Buffer) {
        if buf.version() == self.built_version {
            return;
        }
        self.blocks = if self.kind.is_markdown() {
            crate::parse::parse(buf.content())
        } else {
            Vec::new()
        };
        self.built_version = buf.version();
        self.dirty = true;
    }

    /// The ONLY place child `RevealSm::transition` calls fire. Never bumps
    /// `built_version` — reveal state and content version are deliberately
    /// disjoint (Gotchas).
    pub fn sync_cursors(&mut self, buf: &Buffer, cursors: &CursorSet) {
        let probe = CursorProbe::new(buf, cursors);
        let root_grant = match self.state {
            DocState::Unfocused => RevealGrant::ForceRendered,
            DocState::Focused => RevealGrant::Decide,
        };
        let ctx = InheritCtx {
            focus: self.state,
            wrap: &self.wrap,
            grant: root_grant,
            cursors: &probe,
        };
        let mut dirty = false;
        for b in &mut self.blocks {
            dirty |= b.sync(&ctx);
        }
        self.dirty |= dirty;
    }

    /// `emit` -> wrap (keyed off the root-owned `self.wrap`) ->
    /// `DisplaySnapshot`. The wrap pass runs only here — children never wrap
    /// themselves (plan Context, "Emit -> wrap -> snapshot").
    ///
    /// Memoized on `dirty`: a `view()` call that changed none of
    /// `sync_content`/`set_width`/`sync_cursors`/`set_focus`'s inputs gets a
    /// clone of the last computed `ViewSnapshots` instead of re-running
    /// emit + `WrapMap::sync` + `expand_tables` — commands may call `view()`
    /// (and so `snapshot`) several times per message batch by sanctioned
    /// design, and a keystroke that only moved the cursor touches none of
    /// those inputs.
    pub fn snapshot(&mut self, buf: &Buffer) -> ViewSnapshots {
        if !self.dirty {
            if let Some(cached) = &self.cached {
                return cached.clone();
            }
        }
        let view = self.rebuild(buf);
        self.dirty = false;
        self.cached = Some(view.clone());
        view
    }

    fn rebuild(&self, buf: &Buffer) -> ViewSnapshots {
        #[cfg(test)]
        self.rebuild_count.set(self.rebuild_count.get() + 1);
        let (lines, syntax) = crate::emit::emit(buf.content(), &self.blocks, self.wrap.width);
        let wrap = rune_syntax::wrap::WrapMap::new(self.wrap.width).sync(buf.content(), &lines);
        let display = DisplaySnapshot::from_wrap(&wrap).expand_tables(&wrap);
        ViewSnapshots {
            syntax,
            wrap,
            display,
        }
    }

    /// Bypasses the `snapshot` memo entirely — for verifying
    /// `SYNC-IDEMPOTENT` (a second sync produces the same display as the
    /// first) against a genuine rebuild rather than a memo hit, which would
    /// pass trivially now that `snapshot` caches. Gated on
    /// `strict-invariants` (and test builds) because it exists only for that
    /// verification; production code must always go through `snapshot`.
    #[cfg(any(test, feature = "strict-invariants"))]
    pub fn force_rebuild(&self, buf: &Buffer) -> ViewSnapshots {
        self.rebuild(buf)
    }
}

/// `DocMachine::code_fences`'s own recursion — descends into `Blockquote`
/// and `List` children (a fence can sit inside either), skipping every
/// other block kind since none of them can contain a nested `CodeFence`.
fn collect_code_fences<'a>(blocks: &'a [Block], out: &mut Vec<(&'a str, Vec<Range<usize>>)>) {
    for block in blocks {
        match block {
            Block::CodeFence(cf) => {
                if cf.language.is_empty() || cf.content_lines.is_empty() {
                    continue;
                }
                let lines = cf.content_lines.iter().map(|l| l.start..l.end).collect();
                out.push((cf.language.as_str(), lines));
            }
            Block::Blockquote(bq) => collect_code_fences(&bq.children, out),
            Block::List(list) => {
                for item in &list.items {
                    collect_code_fences(&item.children, out);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_core::cursor::CursorSet;

    #[test]
    fn set_focus_is_idempotent_and_marks_dirty_only_on_change() {
        let mut doc = DocMachine::new();
        assert_eq!(doc.state(), DocState::Unfocused);
        doc.clear_dirty();
        doc.set_focus(false);
        assert!(!doc.is_dirty(), "no-op focus change must not dirty");
        doc.set_focus(true);
        assert!(doc.is_dirty());
        assert_eq!(doc.state(), DocState::Focused);
    }

    #[test]
    fn sync_content_is_a_true_no_op_when_version_is_unchanged() {
        let mut doc = DocMachine::new();
        doc.set_focus(true); // Decide policies only fire when focused.
        let buf = Buffer::new("# hello\n");
        doc.sync_content(&buf);
        assert_eq!(doc.built_version, buf.version());
        assert!(!doc.blocks().is_empty());

        // Reveal the heading (cursor on its line), so its `RevealSm` is now
        // `Revealed` — a state that lives ONLY on the current `blocks` Vec.
        let cursors = CursorSet::new(0);
        doc.sync_cursors(&buf, &cursors);
        assert_eq!(
            doc.blocks()[0].reveal_state(),
            rune_syntax::element::RevealState::Revealed
        );

        // Calling sync_content again with the SAME version must be a true
        // no-op: if it silently reparsed, the freshly-built Heading machine
        // would reset to its default Rendered state, discarding the reveal
        // transition above without ever bumping `built_version` — a `Vec`
        // identity check can't catch this (a Vec can get a new backing
        // allocation with byte-identical contents), but the reveal state
        // survives if and only if no reparse actually happened.
        doc.sync_content(&buf);
        assert_eq!(
            doc.blocks()[0].reveal_state(),
            rune_syntax::element::RevealState::Revealed,
            "sync_content must not reparse when buf.version() is unchanged"
        );
    }

    #[test]
    fn snapshot_short_circuits_when_nothing_changed_between_two_view_calls() {
        // The keystroke-latency regression this test guards: `view()` may be
        // called several times per message batch by sanctioned design, and a
        // cursor-only move changes none of `sync_content`/`set_width`/
        // `sync_cursors`/`set_focus`'s inputs — the second `snapshot` call
        // must be a memo hit, not a second emit + wrap + `expand_tables`.
        let mut doc = DocMachine::new();
        doc.set_focus(true);
        let buf = Buffer::new("# hello\nworld\n");
        let cursors = CursorSet::new(0);

        doc.sync_content(&buf);
        doc.set_width(80);
        doc.sync_cursors(&buf, &cursors);
        let first = doc.snapshot(&buf);
        assert_eq!(doc.rebuild_count(), 1);

        // Same version, same width, same cursor/reveal state: the whole
        // per-message sync sequence again, exactly as `Document::view` would
        // run it for a second call within the same batch.
        doc.sync_content(&buf);
        doc.set_width(80);
        doc.sync_cursors(&buf, &cursors);
        let second = doc.snapshot(&buf);
        assert_eq!(
            doc.rebuild_count(),
            1,
            "a second view() call with no changed input must be a memo hit"
        );
        assert_eq!(first.display.total_rows(), second.display.total_rows());

        // Sanity: a real input change (width) still forces a rebuild.
        doc.set_width(40);
        doc.sync_cursors(&buf, &cursors);
        doc.snapshot(&buf);
        assert_eq!(
            doc.rebuild_count(),
            2,
            "a genuine width change must still force a rebuild"
        );
    }

    #[test]
    fn sync_cursors_never_bumps_built_version() {
        let mut doc = DocMachine::new();
        let buf = Buffer::new("# hello\nworld\n");
        doc.sync_content(&buf);
        let before = doc.built_version;
        let cursors = CursorSet::new(0);
        doc.sync_cursors(&buf, &cursors);
        assert_eq!(doc.built_version, before, "reveal must never bump version");
    }

    #[test]
    fn unfocused_forces_every_decide_policy_block_rendered() {
        // This fixture has only a Heading — a `Decide`-policy block, whose
        // reveal follows `ctx.grant`. It does NOT cover Frontmatter/
        // Verbatim, which are pinned Revealed by design regardless of
        // focus (the reveal-policy table: "Frontmatter, Verbatim | pinned
        // Revealed (no Decide)") — see
        // `frontmatter_and_verbatim_survive_unfocused_as_revealed` below
        // for that intentional exception.
        let mut doc = DocMachine::new();
        let buf = Buffer::new("# hello\n");
        doc.sync_content(&buf);
        // cursor sits on the heading line, which WOULD reveal if focused.
        let cursors = CursorSet::new(2);
        doc.sync_cursors(&buf, &cursors);
        for b in doc.blocks() {
            assert_eq!(
                b.reveal_state(),
                rune_syntax::element::RevealState::Rendered,
                "unfocused doc must force every Decide-policy block Rendered"
            );
        }
    }

    #[test]
    fn frontmatter_and_verbatim_survive_unfocused_as_revealed() {
        // The reveal-policy table's declared exception to "Unfocused ->
        // ForceRendered": Frontmatter and Verbatim (HTML/math/any other
        // unmodeled construct) have no Decide policy at all — they ignore
        // `ctx.grant` entirely and stay pinned Revealed even when the
        // document is unfocused.
        //
        // A table is no longer part of this exception (plan: markdown
        // table rendering, WP1): `Block::Table` now has a real Decide
        // policy (`cursors.any_in_lines(first_line, last_line)`, mirroring
        // `CodeFenceM`), so an unfocused document forces it Rendered like
        // every other Decide-policy block. The fixture below therefore uses
        // an HTML block, which is still a pinned-Revealed `Verbatim`.
        let mut doc = DocMachine::new();
        let buf = Buffer::new("---\ntitle: x\n---\n\n<div>\nraw\n</div>\n");
        doc.sync_content(&buf);
        doc.sync_cursors(&buf, &CursorSet::new(0));
        assert!(
            doc.blocks().len() >= 2,
            "expected a Frontmatter block and a Verbatim (html) block"
        );
        for b in doc.blocks() {
            assert!(
                matches!(
                    b,
                    crate::element::block::Block::Frontmatter(_)
                        | crate::element::block::Block::Verbatim(_)
                ),
                "unexpected block kind in this fixture: {b:?}"
            );
            assert_eq!(
                b.reveal_state(),
                rune_syntax::element::RevealState::Revealed
            );
        }
    }
}
