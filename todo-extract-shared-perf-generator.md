# Extract shared perf generator from build_5k_doc duplication

**Status:** open
**Priority:** low — maintenance debt. The two copies are identical; if one changes without the other, the guard defends a different number than the bench measures.

**Symptom:** none — maintenance debt. The duplication is verbatim, so the numbers match today. Silent divergence would decouple the guard from the bench.

**Root cause:** `build_5k_doc` is duplicated verbatim between `crates/rune-md/benches/parse_bench.rs` and `crates/rune-md/tests/perf_guard.rs`. The guard defends the number the bench measures, so the two must stay in sync.

**Scope:**
- `crates/rune-md/benches/parse_bench.rs` — current location of `build_5k_doc`
- `crates/rune-md/tests/perf_guard.rs` — duplicate of `build_5k_doc`
- New shared location (e.g., `crates/rune-md/src/lib.rs` test helper, or a shared file via `include!`)

**Acceptance criteria:**
- `build_5k_doc` exists in exactly one location and is shared between the bench and the perf guard test.
- The shared generator is accessible from both `benches/` and `tests/` (may require `include!` of a common file or a `#[cfg(any(test, bench))]` helper).
- The bench output and perf guard threshold remain unchanged.
- `make bench` and `make test` pass.

**Notes:**
- The guard defends the number the bench measures. Extracting a shared generator prevents silent divergence.
- The `include!` macro is the simplest approach: place the generator in a shared file (e.g., `crates/rune-md/src/shared_perf.rs` or `crates/rune-md/tests/shared_perf.rs`) and `include!` it from both the bench and the test.
