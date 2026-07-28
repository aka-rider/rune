# Extract shared `build_5k_doc` generator from perf harness

**Status:** open
**Priority:** low — both copies currently match; risk is future silent divergence, not a present bug

**Symptom:** none — maintenance debt. If someone edits the document generator in one file but not the other, the perf guard (tests/perf_guard.rs) and the criterion benchmark (benches/parse_bench.rs) silently measure different documents. The guard's assertion then no longer defends the number the bench actually reports.

**Root cause:** The perf guard was written as a self-contained integration test to avoid pulling the benchmark crate into the test dependency graph. The `build_5k_doc()` function was copied verbatim rather than shared, creating a maintenance burden with no immediate symptom.

**Scope:**
- `crates/rune-md/benches/parse_bench.rs` — criterion benchmark, lines 22-77
- `crates/rune-md/tests/perf_guard.rs` — perf guard test, lines 21-73

Both files define an identical `build_5k_doc() -> String` function (31-line pattern cycled to produce 5,000 lines of mixed markdown). The only differences are cosmetic: the bench version carries inline comments annotating cycle arithmetic.

**Acceptance criteria:**
- [ ] A single source of truth for `build_5k_doc` exists (e.g., a `.rs` file includable via `include!` or a public helper in the crate's test harness)
- [ ] Both `parse_bench.rs` and `perf_guard.rs` reference the same generator; neither defines its own copy
- [ ] `make bench` and `make perf-guard` still compile and pass
- [ ] No new dependencies introduced between the bench and test targets

**Notes:**
- The TODO explicitly says "when next touched" — this is a hygiene item, not a blocker.
- `include!` of a common `.rs` file is the suggested approach since benches and tests are separate compilation targets that may not share a test helper crate easily.
- The shared file should live under `crates/rune-md/` alongside the two consumers (e.g., `benches/shared.rs` or `tests/shared.rs` with a symlink, or a dedicated module under `src/` gated behind a cfg).
