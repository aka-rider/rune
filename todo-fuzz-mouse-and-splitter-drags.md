# Fuzz the splitter drags — `crates/rune-fuzz` has no `Action::Mouse`

**Status:** open
**Priority:** medium — the two draggable splitters (left column and the `Open` divider) are entirely unfuzzed; every other user input the session fuzzer's 27+ invariants defend has some generated action driving it.

**Symptom:** none observed — this is a coverage gap, not a known bug. A splitter drag can put `layout::geometry` into corners a `Resize` alone cannot reach (a drag past a floor mid-frame, a grab-delta computed against a stale geometry, a drag whose pointer leaves the frame entirely), and today nothing generates that sequence.

**Root cause:** `crates/rune-fuzz/src/action.rs` (via `generate/cluster.rs`) enumerates every `Action` the session fuzzer can emit — `Resize` among them, at sizes `1..=200 x 2..=60` — but there is no `Action::Mouse` variant at all, so `commands::splitter::begin`/`commands::splitter::drag` (`crates/rune-tui/src/commands/splitter.rs`) never run under fuzzing.

**Scope:**
- `crates/rune-fuzz/src/action.rs` — add an `Action::Mouse(MouseInput)` variant (or a small enum of `Down`/`Drag`/`Up` steps, since a single splitter gesture is a short *sequence*, not one independent action)
- `crates/rune-fuzz/src/generate/cluster.rs` (or a new sibling generator module) — generate plausible `Down` → `Drag`* → `Up` runs targeting both splitter bands (`geo.left_splitter`, `geo.tabs_divider`), including drags that leave the frame or land exactly on a pane floor
- `crates/rune-fuzz/src/driver/mod.rs` — dispatch the new action the same way `Resize` is dispatched today

**Acceptance criteria:**
- The fuzzer can generate and replay a `Down`/`Drag`/`Up` splitter gesture through the real `commands::mouse::handle` path, not a hand-rolled shortcut.
- `LAYOUT-FITS` (`crates/rune-fuzz/src/invariant/pane.rs`) is already in place and needs no changes to catch what a bad drag sequence would break — it runs on every step via `check_all`, independent of which action produced the step.
- `PANE-NO-BLEED` still holds across a mouse-generated session: a chrome-focused drag must never mutate the active document.
- `make test-fuzz RC=256` (or higher) stays green with the new action mixed in at its default weight.

**Notes:** Recorded during the mouse-resize-splitters work (plan WP7). Filed instead of implemented — WP7 scoped adding `LAYOUT-FITS` and this TODO, not teaching the generator a new action shape.
