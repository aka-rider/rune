# Extract handle_key/handle_editor_key/handle_db_event trio from app.rs

**Status:** open
**Priority:** low — maintenance debt. The file is 779 lines (§1.6 limit 500), 279 lines over budget. This is the seventh consecutive work package to defer the same fix.

**Symptom:** none — maintenance debt. The file compiles and functions correctly; the overage increases review cost and cognitive load.

**Root cause:** `crates/rune-tui/src/app.rs` grew from 500 to 779 lines across multiple work packages (WP3, WP4, WP5, WP7, chrome-parity, space-leader, sequence-capable-keymap, editor-MVP). Each added the minimum fields and wiring required, but the four-stage `handle_key`/`handle_editor_key`/`handle_db_event` trio — the event dispatch logic — is the largest single chunk and has been deferred as the extraction target since the first overage.

**Scope:**
- `crates/rune-tui/src/app.rs` — 779 lines, extract the handler trio
- New file: `crates/rune-tui/src/app_handlers.rs` (or similar name) — receives `handle_key`, `handle_editor_key`, `handle_db_event`
- `crates/rune-tui/src/lib.rs` — add module declaration

**Acceptance criteria:**
- `app.rs` is under 500 lines after the extraction.
- The handler trio (`handle_key`, `handle_editor_key`, `handle_db_event`) moves to the new module with no change to call-site semantics.
- The `App` struct remains in `app.rs` as the primary concern.
- All `pub` symbols retain their existing paths (re-export from `app` if needed).
- `make build`, `make test`, and `make lint` pass.

**Notes:**
- This is the seventh consecutive deferral. The TODO entries recorded the file at 516, 511, 546, 555, 625, 630, 668, 712, 720, 729, and now 779 lines.
- The handler trio is the natural extraction target because it is the largest contiguous block of logic in `app.rs` and has clear boundaries (the four-stage event dispatch).
- The `pane.rs` extraction (WP2) is the reference for how this split should look.
