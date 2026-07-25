//! The root machine: owns focus state and the wrap width every downstream
//! element inherits (plan Context, "Root machine"). `DocMachine::sync_cursors`
//! is the ONLY place child `RevealSm::transition` calls fire; `sync_content`
//! reparses iff the buffer version changed, and reveal transitions never
//! touch `built_version` (Gotchas: "Reveal must never bump the buffer
//! version").

use crate::element::block::Block;
use crate::element::{CursorProbe, InheritCtx, RevealGrant};
use crate::emit::SyntaxSnapshot;
use crate::snapshot::DisplaySnapshot;
use crate::wrap::WrapSnapshot;
use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;

/// `emit` -> wrap (root-owned, keyed off `self.wrap`) -> `DisplaySnapshot`,
/// per `DocMachine::snapshot`. `syntax`/`wrap` carry the coordinate
/// conversions (`buffer_to_syntax`/`syntax_to_buffer`,
/// `syntax_to_wrap`/`wrap_to_syntax`); `display` is the wrap-rows view
/// Phase 5 will later expand for tables/images.
pub struct ViewSnapshots {
    pub syntax: SyntaxSnapshot,
    pub wrap: WrapSnapshot,
    pub display: DisplaySnapshot,
}

/// Whether the editor currently has focus. Unfocused forces every
/// descendant `ForceRendered` — Go's `SyncNoReveal` (Gotchas: "Unfocused ->
/// ForceRendered").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocState {
    Unfocused,
    Focused,
}

/// Root-owned wrap state; only `DocMachine` mutates it. Downstream elements
/// read it through `InheritCtx::wrap`, never own a copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WrapState {
    pub width: u16,
}

impl Default for WrapState {
    fn default() -> Self {
        WrapState { width: 80 }
    }
}

pub struct DocMachine {
    state: DocState,
    wrap: WrapState,
    blocks: Vec<Block>,
    built_version: u64,
    dirty: bool,
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
        }
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

    /// Rebuild the block/inline tree via comrak iff the buffer version
    /// changed. A pure cursor move never bumps `buf.version()`, so this is a
    /// no-op on every keystroke that isn't a content edit.
    pub fn sync_content(&mut self, buf: &Buffer) {
        if buf.version() == self.built_version {
            return;
        }
        self.blocks = crate::parse::parse(buf.content());
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
    pub fn snapshot(&mut self, buf: &Buffer) -> ViewSnapshots {
        let (lines, syntax) = crate::emit::emit(buf.content(), &self.blocks);
        let wrap = crate::wrap::WrapMap::new(self.wrap.width).sync(&lines);
        let display = DisplaySnapshot::from_wrap(&wrap);
        self.dirty = false;
        ViewSnapshots {
            syntax,
            wrap,
            display,
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
    fn sync_content_reparses_only_on_version_change() {
        let mut doc = DocMachine::new();
        let buf = Buffer::new("# hello\n");
        doc.sync_content(&buf);
        let built_after_first = doc.built_version;
        assert_eq!(built_after_first, buf.version());
        assert!(!doc.blocks().is_empty());

        // Same version: sync_content is a no-op (reparse would be visible via
        // a changed Vec identity in a real profiler; here we assert the
        // recorded version is untouched).
        doc.sync_content(&buf);
        assert_eq!(doc.built_version, built_after_first);
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
    fn unfocused_forces_all_blocks_rendered() {
        let mut doc = DocMachine::new();
        let buf = Buffer::new("# hello\n");
        doc.sync_content(&buf);
        // cursor sits on the heading line, which WOULD reveal if focused.
        let cursors = CursorSet::new(2);
        doc.sync_cursors(&buf, &cursors);
        for b in doc.blocks() {
            assert_eq!(
                b.reveal_state(),
                crate::element::RevealState::Rendered,
                "unfocused doc must force every block Rendered"
            );
        }
    }
}
