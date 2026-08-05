# Display-pipeline 100 ms budget violates Non-Blocking Update

**Status:** open
**Priority:** medium — No user-visible symptom yet; the 100 ms budget is a regression guard, not a measured reality. However, on a 5,000+ line document the sync render loop is permitted to block for 100 ms per message batch, which directly tensions the non-blocking update rule. Low priority would imply the tension is acceptable; it is not.

**Symptom:** None observed in practice. The perf guard test `full_pipeline_5k_under_100ms` permits a 100 ms stall without failing, so the test passes even though that budget would produce a visibly laggy keystroke on a large document if the pipeline actually takes near that duration.

**Root cause:** The perf guard test at `crates/rune-md/tests/perf_guard.rs` allows a 100 ms full display-pipeline run on a 5,000-line document. Meanwhile `App::sync_view` in `crates/rune-tui/src/app.rs` runs that pipeline synchronously with no async offload. The runtime loop in `crates/rune-tui/src/runtime.rs` calls `sync_view()` before every frame is drawn — once per processed message batch, not once per message. The non-blocking update rule requires `App::update` to stay non-blocking, with I/O leaving the thread as commands; a 100 ms synchronous stall on the render path violates that contract even though `sync_view` sits just outside `update` in the same synchronous loop.

**Scope:**
- `crates/rune-md/tests/perf_guard.rs` — the `full_pipeline_5k_under_100ms` test and its 100 ms threshold
- `crates/rune-tui/src/app.rs` — `App::sync_view`, the synchronous pipeline invocation
- `crates/rune-tui/src/runtime.rs` — the render loop with two `app.sync_view()` calls per frame
- The non-blocking update rule — the update loop stays non-blocking; I/O leaves the thread as commands

**Acceptance criteria:**
- The display-pipeline recompute no longer blocks the synchronous render loop on large documents.
- The perf guard test either (a) enforces a budget low enough that synchronous execution is imperceptible (single-digit milliseconds), or (b) is replaced/relaxed only if the recompute has moved off the render path.
- If an async worker is introduced, it follows the last-good-result-kept rule (already established for tree-sitter highlighting) — the frame draws with the previous result while the new one computes.
- `App::sync_view` either becomes a thin coordinator that never waits for the full pipeline, or the full pipeline is eliminated from the per-frame path entirely.
- No regression in correctness: the display must never show stale content for more than one frame after a keystroke.

**Notes:**
- Originated as WP11.S6 finding, recorded 2026-07-28.
- The TODO explicitly says "no fix design proposed" — this ticket records the tension without prescribing architecture. Candidates: incremental recompute, or an async worker with last-good-result-kept.
- The tree-sitter highlighting layer already uses an async worker with the last-good-result-kept pattern; that is the reference implementation for whichever approach is chosen.
- Do not treat 100 ms as a UX guarantee. It was set as a regression guard to prevent order-of-magnitude slowdowns; it is not a user-experience target.
