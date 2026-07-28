# Split keymap.rs — extract chord-shape helpers

**Status:** open
**Priority:** low — maintenance debt. The file is 619 lines (§1.6 limit 500), 119 lines over budget.

**Symptom:** none — maintenance debt.

**Root cause:** `crates/rune-tui/src/keymap.rs` grew to 619 lines. The WP5 `GlobalCommand::FocusTitle` + `^r` binding added ~11 lines on top of the existing overage. The file already went through one extraction (`binding.rs`/`global.rs`), but the chord-shape helpers (`resolve_directional`/`resolve_plain_or_shift`/`resolve_vertical`) are still in the main file.

**Scope:**
- `crates/rune-tui/src/keymap.rs` — 619 lines, extract chord-shape helpers
- New file: `crates/rune-tui/src/keymap_resolve.rs` (or similar) — receives the three chord-shape helpers alongside `Command`
- `crates/rune-tui/src/lib.rs` — add module declaration

**Acceptance criteria:**
- `keymap.rs` is under 500 lines after the split.
- The three chord-shape helpers (`resolve_directional`, `resolve_plain_or_shift`, `resolve_vertical`) move to the new module.
- The `Command` enum and its variants stay accessible at their current paths.
- `make build`, `make test`, and `make lint` pass.

**Notes:**
- The `binding.rs`/`global.rs` extraction is the reference for how this split should look.
- The chord-shape helpers are a self-contained unit that could move alongside `Command` itself, mirroring the `binding.rs`/`global.rs` extraction.
