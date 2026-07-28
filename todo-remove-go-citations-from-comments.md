# Remove Go source-location citations from Rust code comments

**Status:** open
**Priority:** low — maintenance debt; no user-facing impact. CLAUDE.md house rule violation (never cite file paths or line numbers in comments), but the citations serve as useful parity references during the active port.

**Symptom:** none — maintenance debt.

**Root cause:** During the rust/golang repository swap (commit 429fb2e, 2026-07-28), ~242 Go-filename citations with line numbers were deliberately left in place rather than rewritten in bulk. Each citation requires a judgment call about what the surrounding sentence is actually claiming, so they were preserved as-is to avoid mangling meaning.

**Scope:** 175 citation lines across 38 Rust source files in `crates/`, spanning all major crates:

- `rune-tui` — the bulk (app, render, breadcrumb, document, footer, pane, save, title, workspace, commands/nav, clipboard, db, runtime, tests/edit_commands)
- `rune-db` — adopt, blob, document, error, journal, load, materialize, observation, payload, probe, reaper, session, snapshot, store, sync, writer
- `rune-core` — buffer, cursor, undo, tests/buffer_roundtrip
- `rune-syntax` — wrap/mod
- `rune-fuzz` — lib, invariant/cursor, invariant/save, invariant/pane
- `rune-md` — tests/wrap_roundtrip

Citation patterns include:
- `edit_primitives.go:51,86` — specific line references
- `workspace_view.go:327-330` — line ranges
- `breadcrumb.go:56-119` — function-level ranges

**Acceptance criteria:**

- Zero remaining `.go:NNN` citations in any `.rs` file under `crates/`.
- Each removed citation replaced with one of:
  - A plain description of the Go behavior being referenced (preferred), when the Rust code already implements it and the comment explains the invariant.
  - A reference to the Go source file name only (no line number), when the reader may need to look up the original for parity verification.
  - Removal entirely, when the citation added no information beyond what the surrounding text already states.
- All affected comments still read correctly and accurately describe the code.
- No behavior changes — this is a comment-only pass.

**Notes:**

- CLAUDE.md house rule: "Never cite a file path, line number, or symbol location in a code comment."
- The TODO item's original instruction was "erase opportunistically when touching the surrounding code." This ticket exists to track and close out the remaining count. It does not need to be done in one sweep; incremental PRs are fine.
- Some citations are bundled with substantive explanations of Go behavior. Those are the ones requiring judgment calls — preserve the explanation, strip the location.
- A mechanical grep to verify completion: `grep -rn '\.go:\d' crates/ --include='*.rs'` should return zero matches.
- Additionally, there are ~17 citations that reference Go files without line numbers (e.g., `keymap.go`, `store.go`). These are acceptable under the house rule and do not need removal unless the surrounding comment is being rewritten for clarity.
