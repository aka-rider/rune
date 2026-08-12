//! The root machine: owns reveal mode and the wrap width every downstream
//! element inherits (plan Context, "Root machine"). `DocMachine::sync_cursors`
//! is the only place child `RevealSm::transition` calls fire during normal
//! editing (`reveal_all` below drives the same recursion off-editor, for the
//! markdown-fence emitter); `sync_content` reparses iff the buffer version
//! changed, and reveal transitions never touch `built_version` (Gotchas:
//! "Reveal must never bump the buffer version").

use std::sync::Arc;

use crate::element::block::Block;
use crate::element::code_region::{self, CodeRegion};
use crate::icons::IconSet;
use crate::snapshot::{DisplaySnapshot, ImageDims};
use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_syntax::SyntaxSnapshot;
use rune_syntax::element::{CursorProbe, InheritCtx, RevealGrant, RevealMode, WrapState};
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
    /// Every region of code in the document this view describes — the one
    /// value the highlight scheduler and the background painter both read,
    /// so neither walks the block tree itself.
    ///
    /// Behind an `Arc` because a `ViewSnapshots` is cloned out of the
    /// `snapshot` memo several times per message batch (commands call
    /// `view()` freely) while it is computed only when the document
    /// actually changes: an owned `Vec` here would trade one walk per frame
    /// for one deep copy per `view()` call, and a whole code document's
    /// region carries one `Range` per buffer line.
    pub code_regions: Arc<[CodeRegion]>,
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
        wrap: &wrap,
        grant: RevealGrant::ForceRevealed,
        cursors: &cursors,
    };
    for b in blocks {
        b.sync(&ctx);
    }
}

pub struct DocMachine {
    reveal_mode: RevealMode,
    wrap: WrapState,
    blocks: Vec<Block>,
    built_version: u64,
    dirty: bool,
    kind: DocumentKind,
    /// Which glyph tier decor producers draw from — set once by the runtime
    /// (plan WP5) via `set_icons`, mirrored here because `Document` (the
    /// caller) holds no reference back to `App`'s theme/terminal state.
    icons: IconSet,
    /// Per-EMBED cell footprint for inline `![alt](x.png)` images inside an
    /// ordinary markdown document, set by the runtime via `set_embed_dims`
    /// — mirrors `icons`' own "the caller pushes terminal-side state in,
    /// `DocMachine` stays terminal-free" shape. Empty by default, which is
    /// exactly `expand_images`'s "no dimensions known yet" case: every
    /// standalone image line still reserves its default 1 row, it just
    /// doesn't grow further until dimensions arrive.
    ///
    /// Distinct from `image_dims` below, which describes a whole image
    /// DOCUMENT rather than an embed within a text one. The two never both
    /// apply: an image document has no embeds, and a markdown document is
    /// never the `Image` kind.
    images: ImageDims,
    /// `(width, rows)` an image DOCUMENT's producer reserves — read only
    /// when `kind == DocumentKind::Image`. `rows` is the number of
    /// synthetic `DisplayRow`s `rebuild` synthesizes in place of the
    /// ordinary emit/wrap pipeline; `width` is carried onto each row's
    /// `ImageRowRef` for the renderer. Defaults to `(0, 1)` — one row,
    /// nothing known about width yet — so an image document that has not
    /// had its dimensions set at all still has exactly one reserved row
    /// rather than zero.
    image_dims: (usize, usize),
    /// The memo `snapshot` returns a clone of when `dirty` is false —
    /// `None` only before the first `snapshot` call. `dirty` is the single
    /// guard: every setter that can change what `snapshot` would compute
    /// (`sync_content` on a version change, `set_width`, `sync_cursors`/
    /// `set_reveal_mode` on a reveal-relevant change) sets it, so a `view()` call
    /// that changed none of those inputs gets the cached clone instead of
    /// re-running emit + wrap + `expand_tables` + the code-region walk.
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
            reveal_mode: RevealMode::Never,
            wrap: WrapState::default(),
            blocks: Vec::new(),
            // `Buffer::version()` starts at 1 (see rune-core), so 0 can never
            // equal a real buffer version — the first `sync_content` call
            // always reparses without a separate "never built yet" flag.
            built_version: 0,
            dirty: true,
            kind: DocumentKind::Markdown,
            icons: IconSet::unicode(),
            images: ImageDims::new(),
            image_dims: (0, 1),
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

    /// The single writer of `reveal_mode` for `DocMachine` — the crate's
    /// other transition writer (`RevealSm::transition` in `element/mod.rs`
    /// is the first; this is the second and last).
    fn transition(&mut self, next: RevealMode) {
        let prev = self.reveal_mode;
        self.reveal_mode = next;
        self.enter_state(prev);
    }

    fn enter_state(&mut self, _prev: RevealMode) {
        self.dirty = true;
    }

    pub fn reveal_mode(&self) -> RevealMode {
        self.reveal_mode
    }

    pub fn wrap(&self) -> WrapState {
        self.wrap
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// Every region of code in this document — the one definition every
    /// downstream consumer reads, whether it highlights the region or merely
    /// paints a background behind it. Private: `rebuild` calls it once per
    /// document change and publishes the result on `ViewSnapshots`, which is
    /// where every consumer reads it from. Walking the tree per consumer (and
    /// so, for the background painter, per frame) is exactly what that
    /// publication exists to prevent.
    ///
    /// What counts depends entirely on `kind`. A `Code` document is exactly
    /// one region spanning the whole buffer; a `Markdown` document is one
    /// region per fenced block (found recursively, so a fence nested in a
    /// blockquote or list item counts) plus every indented code block; a
    /// `Plain` or `Image` document has none. A region whose `info` is empty
    /// is still returned — see `CodeRegion` for why, and for why `content`
    /// is one range per physical line rather than one contiguous span.
    ///
    /// Takes the buffer rather than reading a mirrored copy: `DocMachine`
    /// owns no buffer (every content-reading method here is handed one), and
    /// a `Code` document's regions come from the buffer's own line structure
    /// because a non-markdown kind is never parsed into `blocks` at all.
    fn code_regions(&self, buf: &Buffer) -> Arc<[CodeRegion]> {
        match self.kind {
            DocumentKind::Code(lang) => Arc::from([code_region::whole_document(lang.name(), buf)]),
            DocumentKind::Markdown => {
                let mut out = Vec::new();
                code_region::collect(&self.blocks, buf, &mut out);
                Arc::from(out)
            }
            DocumentKind::Plain | DocumentKind::Image => Arc::from([]),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// `has_insertion_point` is `Document::has_insertion_point()` — whether
    /// this document currently has a live insertion point to reveal at, not
    /// whether its pane has focus (a focused-but-read-only document has no
    /// insertion point either).
    pub fn set_reveal_mode(&mut self, has_insertion_point: bool) {
        let next = if has_insertion_point {
            RevealMode::AtCursor
        } else {
            RevealMode::Never
        };
        if next != self.reveal_mode {
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

    /// Per-embed dimensions for the inline images inside a text document;
    /// marks dirty but fires NO reveal transitions, the same shape
    /// `set_icons`/`set_width` already use for terminal-side state that
    /// isn't a content edit or a focus/reveal change.
    ///
    /// Named for EMBEDS specifically to keep it distinct from
    /// `set_image_document_dims`, which sizes a whole image document.
    pub fn set_embed_dims(&mut self, dims: ImageDims) {
        if self.images != dims {
            self.images = dims;
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

    /// The reserved `(width, rows)` an image DOCUMENT's producer
    /// synthesizes rows for — a no-op for any other `kind`. `rows` is
    /// floored at 1 by `DisplaySnapshot::image_rows` itself, so passing `0`
    /// here is safe. Marks dirty only on an actual change, same memoization
    /// shape as `set_width`/`set_icons`.
    ///
    /// Distinct from `set_embed_dims`, which sizes the inline images within
    /// a text document rather than the document itself.
    pub fn set_image_document_dims(&mut self, width: usize, rows: usize) {
        let dims = (width, rows);
        if dims != self.image_dims {
            self.image_dims = dims;
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
        let root_grant = match self.reveal_mode {
            RevealMode::Never => RevealGrant::ForceRendered,
            RevealMode::AtCursor => RevealGrant::Decide,
        };
        let ctx = InheritCtx {
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
    /// `sync_content`/`set_width`/`sync_cursors`/`set_reveal_mode`'s inputs gets a
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
        // `emit`/`wrap` still run even for `DocumentKind::Image` below —
        // cheaply, since an image document's buffer is always empty — so
        // `syntax`/`wrap` stay valid coordinate maps for the rest of the
        // pipeline (cursor sync, etc.) exactly like every other kind. Only
        // `display` diverges: the image producer (plan WP4.S2) synthesizes
        // its rows directly rather than deriving them from `wrap`, since an
        // empty buffer has no wrap rows to derive an image's reserved
        // layout from at all.
        let (lines, syntax) = crate::emit::emit_with(
            buf.content(),
            &self.blocks,
            self.wrap.width,
            &self.icons,
            crate::emit::style::base_scope(self.kind),
        );
        let wrap = rune_syntax::wrap::WrapMap::new(self.wrap.width).sync(buf.content(), &lines);
        // An image document synthesizes its rows outright — there is no
        // buffer text to emit or wrap. Every other kind goes through the
        // ordinary pipeline, where `expand_images` reserves rows for the
        // inline embeds a markdown document may contain.
        let display = if self.kind == DocumentKind::Image {
            DisplaySnapshot::image_rows(self.image_dims.1, self.image_dims.0)
        } else {
            DisplaySnapshot::from_wrap(&wrap)
                .expand_tables(&wrap)
                .expand_images(&wrap, &self.blocks, buf.content(), &self.images)
        };
        ViewSnapshots {
            syntax,
            wrap,
            display,
            code_regions: self.code_regions(buf),
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
    /// that only needs the cache-bypassed rebuild is not forced to also
    /// arm every `assert_invariant` in the crate just to reach it.
    #[cfg(any(test, feature = "strict-invariants", feature = "fuzz-hooks"))]
    pub fn force_rebuild(&self, buf: &Buffer) -> ViewSnapshots {
        self.rebuild(buf)
    }
}

#[cfg(test)]
#[path = "doc_tests.rs"]
mod tests;
