# Reduce or offload the 100 ms display-pipeline budget that blocks the synchronous render loop

**Status:** open
**Priority:** low — no user-visible lag observed; this is a disclosed architectural tension between an existing perf-regression guard and §5.3, not a regression or bug. The 100 ms ceiling is a regression fence, not a UX guarantee.

**Symptom:** None observed. The perf guard `full_pipeline_5k_under_100ms` permits a full display-pipeline recompute to take 100 ms on a 5,000-line document. Because `App::sync_view` runs this pipeline synchronously from the runtime's blocking message loop before every frame is drawn, that budget allows up to 100 ms of stall on each keystroke when editing a large document.

**Root cause:** The display pipeline (parse → syntax-highlight → wrap → snapshot) runs entirely on the render-loop thread via `App::sync_view`, which the runtime calls once per processed message batch — not once per message. §5.3 requires `Update()` and `Init()` to stay non-blocking; while `sync_view` sits just outside `update` in the same synchronous loop, a 100 ms stall per keystroke violates the spirit of the constraint. The perf guard test codifies the 100 ms ceiling as acceptable, masking the tension.

**Scope:**
- `crates/rune-md/tests/perf_guard.rs` — `full_pipeline_5k_under_100ms` sets the 100 ms regression budget
- `crates/rune-tui/src/app.rs` — `App::sync_view` runs the pipeline synchronously
- `crates/rune-tui/src/runtime.rs` — the Elm runtime loop calls `sync_view` before each frame draw
- `crates/rune-md` — the display pipeline itself (parse, highlight, wrap, snapshot)

**Acceptance criteria:**
- [ ] The synchronous render loop no longer blocks for the full cost of a display-pipeline recompute on every keystroke
- [ ] The `full_pipeline_5k_under_100ms` perf guard is either tightened to reflect a realistic UX budget, or replaced/supplemented with an async-equivalent guard if the pipeline moves off-thread
- [ ] The "last-good-result-kept" pattern (already used for tree-sitter highlighting) applies: the display shows the previous frame's result while the new recompute runs, with no flicker or stale-region artifacts
- [ ] §5.3 compliance is verifiable: `Update()` and the render loop remain non-blocking even on documents exceeding 5,000 lines
- [ ] No regressions in existing behavior: keystrokes feel responsive on small documents; large documents render correctly once the async recompute catches up

**Notes:**
- The TODO item explicitly says "recording the tension, not prescribing a redesign." This ticket captures the tension as actionable work.
- Two approaches are mentioned as starting points: (a) incremental recompute (only reprocess changed regions), or (b) async worker with last-good-result-kept. Approach (b) aligns with the existing tree-sitter pattern and is likely simpler to implement correctly.
- Before treating 100 ms as anything other than a regression fence, measure actual latency on representative documents to establish a baseline.
- The Go reference implementation may handle this differently; check `golang/` for comparison if needed.
