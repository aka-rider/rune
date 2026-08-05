# Fix residual double-claim producer bugs in fence/blockquote compositions

**Status:** open
**Priority:** low

The fuzz harness found 4 double-claim violations per 214k documents in shipped builds (182 per 214k under strict-invariants). These are 1-byte caret skews that only manifest when the cursor is focused on the affected line. The existing `push_span_split_by_line` chokepoint in `emit/mod.rs` already absorbs them gracefully in shipped builds (skips the already-claimed byte), so no user-facing panic or data loss occurs. However, the producer itself is still wrong, and the strict-invariants CI will fail.

**Symptom:** When the cursor sits on a specific line in a fence-inside-blockquote or backtick-inside-blockquote composition, the caret position skews by exactly 1 byte. The visible text renders correctly because the single-claim chokepoint silently skips the duplicated byte. Under `STRICT_INVARIANTS` (test builds or the opt-in `strict-invariants` feature), the process panics with an assertion failure in `push_span_split_by_line`.

**Root cause:** Two independent AST nodes (a block-level element's marker range and an inline element's range, or two sibling nodes) both claim the same byte in the source. This is a producer bug in `rune-md/src/parse/` — the parse layer derives a byte range that overlaps with a range another node already owns. The specific compositions that trigger this involve fence markers and backtick delimiters interacting with blockquote container prefixes (the `"> "` marker bytes), where the inline layer's range arithmetic fails to account for a container prefix it inherits from a parent blockquote. Minimal repros discovered by the fuzz shrinker include:

- `">c\n`\n>"` — a backtick on a lazy-continuation line (no `"> "` prefix) followed by a bare `">"` on the next line, creating a 1-byte overlap between the inline code close delimiter and the blockquote marker.
- `"t\n  -```\n*```\n>"` — a fence inside a list item nested under a blockquote, where the fence's close delimiter range collides with the blockquote's marker on the following line.

The exact location is not yet narrowed; it is either in `parse/inline.rs` (inline code close delimiter location, similar to the already-fixed bug documented in `conceal_roundtrip.rs` at the `inline_code_close_delimiter_is_located_not_computed_arithmetically` test) or in `parse/block.rs` (fence close delimiter range derivation).

**Scope:**
- `crates/rune-md/src/parse/inline.rs` — inline code delimiter range derivation
- `crates/rune-md/src/parse/block.rs` — fence close delimiter range derivation
- `crates/rune-md/src/parse/mod.rs` — shared range arithmetic helpers
- `crates/rune-md/tests/conceal_roundtrip.rs` — regression tests (already 1407 lines, deferred split per separate TODO item)
- The repro/shrinker harnesses that discovered these live in scratchpad probe-strict test code from the stage-2b review; they need to be extracted into permanent regression tests.

**Acceptance criteria:**
- The two minimal repros (`">c\n`\n>"` and `"t\n  -```\n*```\n>"`) parse without triggering `assert_invariant` under `STRICT_INVARIANTS`.
- The fuzz harness (214k fresh-seed documents) reports zero double-claim violations in both shipped and strict builds — or, if the remaining panics are confirmed to be the upstream comrak sourcepos inconsistency class already tracked in `crates/rune-md/TODO.md`, the count matches the known comrak-only baseline.
- Permanent regression tests in `conceal_roundtrip.rs` pin both repros with descriptive names, following the pattern of existing tests like `fence_inside_blockquote_container_prefix_not_double_hidden`.
- The scratchpad probe-strict harnesses that discovered these are either folded into the permanent test suite or documented as ephemeral (with the repro strings preserved in the permanent tests).

**Notes:**
- This is distinct from the comrak sourcepos self-inconsistency class tracked in `crates/rune-md/TODO.md`. That class involves comrak itself returning overlapping sourcepos for sibling nodes (an upstream bug). This ticket is about bugs in *our* range derivation logic that could be fixed by correcting the producer.
- The `conceal_roundtrip.rs` file is already flagged for splitting (1407 lines vs 500-line limit). Adding new tests here is acceptable; the split is a separate deferred item.
- The fix should follow the same pattern as the round 9 fixes: locate the delimiter by scanning the actual source bytes rather than computing its position arithmetically from the outer range. See the `trailing_backtick_run` function in `parse/inline.rs` for the established approach.
- Byte-verbatim round-trip is the constitutional backing: a double-claim means the same source byte appears in two SyntaxSpans, which can cause the emitted visible text to either duplicate or drop that byte depending on which span wins.
