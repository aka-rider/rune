# Skip empty edit batches at the commit chokepoint

**Status:** open
**Priority:** low — a papercut that affects both implementations. The user sees "unsaved changes" on a clean file after cutting with nothing to cut; undo clears it. No data loss, just confusing UX.

**Symptom:** When the user presses cut (⌘X) with nothing to cut — empty last line, empty buffer, or no selection — the action journals a zero-width step, bumps the buffer version, and marks the document dirty. A previously clean file then shows "unsaved changes" in the footer until the user presses undo. The undo clears the spurious dirty state.

**Root cause:** The edit commands (cut, and potentially others) construct an `Edit` with `start == end` and `insert` empty when there's nothing to cut. This zero-width edit is passed through to `Buffer::apply_edits`, which correctly returns early with no changes — but the caller (higher in the stack, in the document or journal layer) has already decided to mark the document dirty and bump the version before the edit reaches the buffer. Go's `ApplyEdits` (`golang/pkg/editor/buffer/buffer.go:115-145`) likewise doesn't skip zero-width batches.

**Scope:**
- Rust: `crates/rune-tui/src/commands/clipboard.rs` (cut command), `crates/rune-tui/src/document.rs` (dirty flag and version bump logic), `crates/rune-core/src/buffer.rs` (`apply_edits` already returns early for empty edits but the dirty flag is set upstream)
- Go: `golang/pkg/ui/components/textedit/commands_clipboard.go` (cut command), `golang/pkg/editor/buffer/buffer.go` (`ApplyEdits`)

**Acceptance criteria:**
- Cutting with nothing to cut (no selection, empty buffer, empty last line) does not mark the document dirty, does not bump the version, and does not journal a step.
- The fix is at the commit chokepoint — the layer that decides whether to mark dirty and journal — not in each individual edit command.
- Both Rust and Go implementations are fixed to match.
- Existing tests for normal cut operations continue to pass.
- A regression test binds an empty selection and asserts that cut produces no dirty state, no version bump, and no journal entry.

**Notes:**
- The TODO says "skip empty edit batches at the commit chokepoint." This is the right place — a single guard that filters out zero-width edits before they reach the journal or dirty-flag logic.
- The buffer layer already returns early on empty edits. The issue is the layer above that marks dirty before the edit reaches the buffer.
- Inherited from Go — both implementations share this behavior, so fixing both maintains parity.
