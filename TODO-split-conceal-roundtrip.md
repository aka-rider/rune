# Split conceal_roundtrip.rs into focused test modules

**Status:** open
**Priority:** low — maintenance debt, no user-visible impact. The file compiles and all tests pass; the violation is against the internal house rule in CLAUDE.md that caps source files at 500 lines.

**Symptom:** none — maintenance debt.

**Root cause:** The file grew organically as fuzz-driven verification rounds (rounds 1 through 9) each appended their regression cases as a new section. The original design called for splitting when "next touched," but the file was never revisited for that purpose. It currently sits at 1432 lines, nearly 3x the §1.6 limit.

**Scope:**

- `crates/rune-md/tests/conceal_roundtrip.rs` (1432 lines) — the file to split.

The file contains three top-level concerns interleaved into one:

1. **Reveal-parity table tests** — sections (a) through (a14), covering focused unit tests for each element type's reveal/conceal behavior and every regression found during fuzz verification. This is the bulk of the file.
2. **SyntaxSnapshot round-trip proptest** — section (b), the proptest harness mirroring Go's `FuzzSyntaxMapRoundtrip`, with its own generator logic and blockquote wrapper helpers.
3. **Single-transition-writer grep gate** — section (c), a compile-time/source-level structural invariant check.

Shared helpers used across sections: `synced()` (constructs Buffer + DocMachine) and `joined_line()` (reconstructs line text from spans), plus per-section assertion helpers like `assert_full_line_coverage`, `assert_container_fence_invariants`, `assert_no_duplicate_content`, `assert_wikilink_label`, and `assert_reveal_conceal_coverage`.

**Acceptance criteria:**

- No resulting file exceeds 500 lines.
- All existing tests continue to pass (`make test` is green).
- The shared helpers (`synced`, `joined_line`) are either moved to a shared module within `rune-md/tests/` or kept in a single small file that the others import.
- Each regression section (a2)–(a14) is grouped under the topic it tests (e.g., coverage regressions, container-nesting regressions, comrak line-index regressions) rather than remaining in chronological "verification round" order.
- The proptest in section (b) lives in its own file; it has a distinct dependency on `proptest` and a different test shape.
- The grep gate in section (c) lives in its own file; it reads source code rather than exercising the emit pipeline.
- The `#![allow(clippy::...)]` attribute at the crate root is preserved or scoped appropriately to the test files that need it.

**Notes:**

- The file is a regression archive. Each section documents a specific bug found during fuzzing, with the fixture input pinned verbatim. Do not lose the historical context in the comments — they explain why each test exists and which § of the constitution it guards.
- The per-section assertion helpers (e.g., `assert_full_line_coverage`, `assert_container_fence_invariants`) are tightly coupled to their section's tests and should travel with them rather than being centralized.
- This is a pure refactor. Do not change test logic, fixtures, or assertions.
