# File upstream comrak issue for sourcepos self-inconsistency under lone-CR and tab interactions

**Status:** open
**Priority:** low — all 20 occurrences (18 strict panics + 2 shipped violations per 214k fuzz docs) degrade gracefully in production builds. The `STRICT_INVARIANTS` guards in `emit/mod.rs` skip already-claimed bytes and merge overlapping hidden ranges, so there is no user-facing panic or data loss. This is an upstream correctness bug in comrak's AST, not a rune defect.

**Symptom:** In strict-invariant builds (`cfg(test)` or the `strict-invariants` feature flag), the display pipeline panics when comrak returns sibling AST nodes whose `Sourcepos` byte ranges overlap or contradict each other — a child `Text("x")` whose range lands on top of sibling `Text("]")`'s range, or an `HtmlBlock` whose end extends into a byte a separate top-level `Paragraph` independently claims. In shipped builds, the same inputs are silently absorbed with no visible effect.

**Root cause:** comrak's internal column arithmetic for tab-stop expansion and line-continuation tracking is inconsistent when a lone carriage return (`\r`) or a raw tab character appears inside nested container structures (blockquote within list item, HTML block type 7 with `\r`-terminated lines). The tab-stop expansion appears to be applied at different container depths — once in some paths, twice in others — producing source positions that are mutually contradictory before rune touches them. This is not a case of rune mis-deriving ranges from a reliable sourcepos; comrak itself hands back overlapping sibling claims.

**Scope:**
- `crates/rune-md/src/emit/` — the `push_span_split_by_line` and `build_line_conversions` graceful-degradation paths that currently absorb the inconsistency.
- `crates/rune-md/TODO.md` — contains the three minimal repros and full analysis.
- Upstream `comrak` repository — the actual fix location.
- Any strict-invariants CI job — must remain non-blocking (or exclude the three repros) until comrak is fixed.

**Acceptance criteria:**
- Three minimal repro cases from `crates/rune-md/TODO.md` are further reduced to the smallest possible input that still triggers the inconsistency.
- A single issue is filed against the `comrak` upstream repository containing all three minimized repros, a clear description of the overlapping sourcepos bug, and steps to reproduce.
- The strict-invariants CI job (if one exists) is either marked non-blocking or configured to exclude the three repro cases, so the CI does not red-out on upstream bugs.
- The TODO entry in `TODO.md` (rust port — deferred hygiene items, line 22) is updated to reference this ticket and the upstream issue URL.
- No changes to production code are required; this ticket is complete when the upstream issue is filed and the CI is gated accordingly.

**Notes:**
- The three minimal repros are documented in `crates/rune-md/TODO.md`:
  1. `"- >_\r\tx\n]"` — tab after lone `\r` inside blockquote nested in list item, on a lazy-continuation line.
  2. `">\t<b>\ra"` — tab after blockquote marker, HTML block, then lone `\r`; HtmlBlock end bleeds into sibling Paragraph byte.
  3. `"- >👍\n\tx\nc"` — same list+blockquote+tab shape without `\r`, confirming tab-stop expansion alone triggers it.
- A post-parse reconciliation pass (detecting and clipping overlapping sibling ranges) is explicitly out of scope — it risks silently swallowing real content at the seam and is worse than the current graceful degradation. Only consider it if the upstream issue goes unfixed for an extended period and the violation rate increases.
- The existing safety net (`assert_invariant` gated behind `STRICT_INVARIANTS`) is sufficient for the foreseeable future; do not weaken or remove it.
