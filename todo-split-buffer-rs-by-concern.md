# Decompose buffer.rs into concern-aligned modules

**Status:** open
**Priority:** low — buffer.rs sits at 617 lines against the §1.6 limit of 500. The overage is pre-existing (was already 568 before WP2.S6 added ~49 lines for `display_position` and its tests). No user-facing symptom; it is maintenance debt that increases review cost and cognitive load on future edits.

**Symptom:** none — maintenance debt

**Root cause:** `crates/rune-core/src/buffer.rs` accumulated three distinct concerns as the Rust port progressed: the core `Buffer` type with its edit and coordinate methods, the `Edit`/`AppliedEdit`/`BufferError` types that are conceptually the edit vocabulary, and the line-index computation helpers (`compute_line_starts`, `find_line`, `compute_added_starts`). The §1.6 rule ("one primary type per file; decompose any file past 500 LoC") was violated before the `display_position` chokepoint was added, and that addition pushed it further over.

**Scope:**
- `crates/rune-core/src/buffer.rs` (617 lines, the file to decompose)
- `crates/rune-core/src/lib.rs` (module declarations to add)
- Any module that imports from `buffer.rs` (re-exports or path updates)

**Acceptance criteria:**
- No source file in `crates/rune-core/src/` exceeds 500 lines after the split.
- The `Edit`, `AppliedEdit`, `BufferError` types and their free functions (`is_sorted_descending_non_overlapping`, `clone_and_sort_edits_descending`) are extracted to a dedicated module (e.g., `edit.rs`). These are the edit vocabulary; they are already conceptually separate from `Buffer` and are used by callers outside the buffer module.
- The line-index helpers (`compute_line_starts`, `find_line`, `compute_added_starts`, `debug_assert_line_starts_invariant`) remain private to the buffer module or move with the `Buffer` type, since they only serve `Buffer` internally.
- The `Buffer` struct, its impl block, `Default` impl, and `line_starts` invariant stay together in `buffer.rs` as the primary concern.
- All `pub` symbols retain their existing re-export paths from `crate::buffer` (or are re-exported from `lib.rs`) so downstream modules require no import changes.
- `make build`, `make test`, and `make lint` pass with no new warnings.
- The `#[cfg(test)]` module travels with the code it tests; no test is orphaned.

**Notes:**
- The Go reference already has this split: `buffer.go` (Buffer type), `textedit/edit_primitives.go` (Edit vocabulary), `lineindex.go` (line index). Follow that boundary.
- The `display_position` method and its three unit tests are the reason the file crossed 568 lines; they stay with `Buffer` since they are a Buffer query method.
- CONSTITUTION.md §1.6: "One primary type per file; decompose any file past 500 LoC." This ticket enforces that article.
