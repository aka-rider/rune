# Skip zero-width edit batches at the commit chokepoint

**Status:** open
**Priority:** low — cosmetic papercut; no data loss, no functional break, but every affected user will see it

**Justification:** The bug is confined to edge-case interactions (cut on an empty buffer or empty last line). It does not affect normal editing, does not corrupt data, and the spurious dirty state clears on undo. However, it degrades trust in the "unsaved changes" indicator and inflates the undo journal with no-op steps.

---

## Symptom

Pressing cut (or any edit command) when there is nothing to cut — such as on an empty buffer or an empty last line — journals a zero-width step, bumps the buffer version, and marks the document dirty. A previously clean file then shows "unsaved changes" in the status bar until the user undoes the no-op.

## Root cause

Edit commands derive a batch of `Edit` structs from cursor positions. When the cursor has no selection and the "bare" range closure returns a zero-width span (e.g., `start == end == 0` on an empty buffer), the resulting edit `{start: 0, end: 0, insert: ""}` is a no-op but is not filtered out. The commit chokepoint only guards against a literally empty `infos` vec, not against a vec of no-op edits. The batch passes through to `Buffer::apply_edits`, which bumps `version` unconditionally, and the journal is pushed with a step that changes nothing.

The same gap exists in Go: `ApplyEdits` returns early only for `len(edits) == 0`, and `commitEdits` unconditionally bumps `m.rev++`.

## Scope

### Rust

- `crates/rune-tui/src/commands/edit_core.rs` — `apply_edit_batch_with_cursors()` at the `infos.is_empty()` guard. This is the single chokepoint; adding a filter here covers every editing command (typing, backspace, cut, paste, line operations).
- `crates/rune-core/src/buffer.rs` — `apply_edits()` bumps `version` on every non-empty call. A secondary (defensive) guard could go here, but the primary fix belongs at the TUI chokepoint above where the journal push happens.

### Go

- `golang/pkg/ui/components/textedit/textedit.go` — `applyOperation` at the `len(result.Operation.Edits) > 0` guard (line ~374). Filter out no-op edits before calling `ApplyEdits`.
- `golang/pkg/editor/buffer/buffer.go` — `ApplyEdits` bumps `version` (line ~179). Secondary defensive guard possible.

### Shared

- Both implementations need a unit test asserting that a zero-width edit batch on an empty buffer does not bump version, does not journal a step, and does not mark the document dirty.

## Acceptance criteria

- [ ] Cutting on an empty buffer does not journal a step, does not bump the buffer version, and does not mark the document dirty (Rust).
- [ ] Cutting on an empty buffer does not journal a step, does not bump `rev`, and does not mark the document dirty (Go).
- [ ] Cutting on an empty last line (cursor at EOF, no selection, line is `""`) has the same no-op behavior.
- [ ] A batch containing a mix of real edits and zero-width no-op edits still applies the real edits correctly (the filter must not discard the entire batch).
- [ ] Existing test suites pass; no regression in normal editing, undo/redo, or dirty tracking.
- [ ] Unit test added in both implementations: "zero-width edit batch is a no-op" covering empty buffer and empty last line scenarios.

## Notes

- The fix is a filter, not a validation change. An edit with `start == end && insert.is_empty()` is a valid no-op (it represents "replace nothing with nothing"), so `apply_edits` is correct to accept it. The bug is that the caller should not have committed it in the first place.
- The Go TODO (`golang/TODO.md`) should also be updated once fixed, since the item currently lives in the Rust TODO as a cross-implementation item.
- Related: the `per_cursor_selection_edits` helper in `edit.rs` already skips cursors with no selection and no bare range (the `continue` at the `bare` closure's `None` branch). The gap is specifically when `bare` returns `Some((n, n))` — a zero-width range that produces a no-op edit. Consider whether `bare` closures should be responsible for returning `None` in that case, but the chokepoint filter is the stronger invariant (CONSTITUTION §1.3: find the root cause, not a spot check).
