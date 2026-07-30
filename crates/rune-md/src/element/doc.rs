//! The root machine: owns focus state and the wrap width every downstream
//! element inherits (plan Context, "Root machine"). `DocMachine::sync_cursors`
//! is the only place child `RevealSm::transition` calls fire during normal
//! editing (`reveal_all` below drives the same recursion off-editor, for the
//! markdown-fence emitter); `sync_content` reparses iff the buffer version
//! changed, and reveal transitions never touch `built_version` (Gotchas:
//! "Reveal must never bump the buffer version").

use std::ops::Range;

use crate::element::block::Block;
use crate::icons::IconSet;
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

/// Force every block (and, transitively, every nested block/inline) into
/// `RevealState::Revealed`, for the WP6 markdown-fence emitter: fence
/// bodies always emit at full reveal regardless of any cursor, so the
/// overlay carries real syntax colors rather than the folded form. Reuses
/// `DocMachine::sync_cursors`'s own recursion (a root `ForceRevealed` grant
/// with an empty cursor probe, `InheritCtx::child` propagating the force to
/// every descendant) instead of writing `RevealState` fields directly —
/// `RevealSm::transition` stays the sole writer of reveal state in the
/// crate.
pub fn reveal_all(blocks: &mut [Block]) {
    let wrap = WrapState::default();
    let cursors = CursorProbe::default();
    let ctx = InheritCtx {
        focus: DocState::Focused,
        wrap: &wrap,
        grant: RevealGrant::ForceRevealed,
        cursors: &cursors,
    };
    for b in blocks {
        b.sync(&ctx);
    }
}

pub struct DocMachine {
    state: DocState,
    wrap: WrapState,
    blocks: Vec<Block>,
    built_version: u64,
    dirty: bool,
    kind: DocumentKind,
    /// Which glyph tier decor producers draw from — set once by the runtime
    /// (plan WP5) via `set_icons`, mirrored here because `Document` (the
    /// caller) holds no reference back to `App`'s theme/terminal state.
    icons: IconSet,
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
            icons: IconSet::unicode(),
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

    /// Icon-tier change (plan WP5); marks dirty but fires NO reveal
    /// transitions, mirroring `set_width`'s memoization shape exactly — a
    /// terminal-capability change is neither a content edit nor a
    /// focus/reveal change.
    pub fn set_icons(&mut self, icons: IconSet) {
        if self.icons != icons {
            self.icons = icons;
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
        if !self.dirty
            && let Some(cached) = &self.cached
        {
            return cached.clone();
        }
        let view = self.rebuild(buf);
        self.dirty = false;
        self.cached = Some(view.clone());
        view
    }

    fn rebuild(&self, buf: &Buffer) -> ViewSnapshots {
        #[cfg(test)]
        self.rebuild_count.set(self.rebuild_count.get() + 1);
        let (lines, syntax) =
            crate::emit::emit_with(buf.content(), &self.blocks, self.wrap.width, &self.icons);
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
    /// pass trivially now that `snapshot` caches. Gated on `strict-
    /// invariants`/`fuzz-hooks` (and test builds) because it exists only
    /// for that verification; production code must always go through
    /// `snapshot`. `fuzz-hooks` is a SEPARATE feature from `strict-
    /// invariants` (not implied by it either way) precisely so a consumer
    /// like rune-fuzz can reach this hook without also inheriting this
    /// crate's known-open comrak-sourcepos `assert_invariant` panics
    /// (`strict-invariants`'s own docs, `TODO.md`).
    #[cfg(any(test, feature = "strict-invariants", feature = "fuzz-hooks"))]
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
#[path = "doc_tests.rs"]
mod tests;
