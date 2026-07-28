# Minimize comrak strict-invariant repros and file upstream; keep CI non-blocking

**Status:** open
**Priority:** low — upstream bugs block perfect invariant coverage but do not affect user-facing behavior; the strict-invariants job is already excluded or marked non-blocking as a stopgap.

**Symptom:** none — maintenance debt. The strict-invariants CI job either fails on known comrak-produced sourcepos bugs or must be excluded to keep the green bar.

**Root cause:** comrak has unresolved issues with source position tracking for certain markdown constructs. Rune's strict-invariant checks (which verify that edit ranges, byte offsets, and display geometry stay consistent) hit these upstream bugs. Until comrak fixes propagate, the invariants will fire false positives on specific inputs.

**Scope:**
- CI configuration — wherever the strict-invariants job is defined (GitHub Actions workflow or `make` target).
- The three known repro inputs that trigger the invariant failures.
- Any test or fuzz corpus entries that exercise the affected comrak paths.

**Acceptance criteria:**
- Each of the three repros has been minimized to the smallest input that still triggers the comrak bug.
- Minimal repros are filed as issues against the comrak upstream repository (links recorded here or in the issue tracker).
- The strict-invariants CI job is explicitly marked as non-blocking (or the three repros are excluded from the corpus) with a comment linking to the upstream issues, so the rationale is clear to future maintainers.
- When comrak ships fixes, removing the non-blocking flag or re-including the repros restores full invariant coverage without further changes.

**Notes:**
- Do not disable the strict-invariants checks entirely — they catch real bugs in rune's own code. Only the known comrak-triggered false positives should be excluded or deprioritized.
- The Go reference implementation does not have this problem because it uses a different parsing strategy; the repros are rust/comrak-specific.
- Watch for comrak releases that address sourcepos accuracy; this ticket should be closed when the upstream fixes are available and the CI guard can be removed.
