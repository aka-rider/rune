# Report comrak wikilink line-counter desync upstream; remove local workaround

**Status:** open
**Priority:** low — The workaround is stable, well-tested, and only triggers on a rare edge case (wikilinks spanning multiple physical lines). No user-facing impact while the workaround is in place. Reporting upstream benefits the broader comrak ecosystem; removing the workaround simplifies the codebase.

---

### Symptom

When a wikilink's match spans a raw newline or carriage return (e.g. `[[\n]]` or `[[\r]]`), comrak 0.54's internal line counter shifts for all subsequent inline nodes in the same paragraph. Downstream siblings report wrong physical line numbers in their `sourcepos`, causing their byte ranges to compute earlier than their true position — landing on top of an earlier sibling's already-claimed bytes. The same corruption propagates to parent wrappers (Emphasis, Strikethrough, Link) that read a corrupted last-child's sourcepos to place their close delimiter via `child_gap_delims`.

### Root Cause

comrak's wikilink extension performs inline substitution that advances its own line counter past any embedded newline within the wikilink match, but does not account for that when computing `sourcepos` line numbers for subsequent inline siblings. The base emphasis/strong pass runs before the wikilink substitution and assigns correct outer `sourcepos` to wrappers — so only the internal structure of affected nodes is corrupted, not their outer boundaries.

This was discovered through verification rounds 3-4 of the conceal roundtrip test suite, which caught duplicate-claim violations under strict per-line byte coverage invariants.

### Scope

**Upstream comrak:** The wikilink extension's inline parsing and `sourcepos` line-number assignment. Affects comrak 0.54.

**Local workaround (crates/rune-md):**

- `src/parse/inline.rs` — `subtree_has_multiline_wikilink()` (detection) and `build_inlines()` (per-line rebuild recovery). When a multiline wikilink is detected in a child's subtree, the workaround abandons comrak's sourcepos for that node and all remaining lines in the paragraph, reconstructing content as plain `TextRun` pieces keyed off `ScanHint` instead. After the corrupted node, it returns early, skipping normal inline processing for remaining siblings.
- `src/parse/mod.rs` — `LineIndex` dual-index infrastructure (`comrak` vs `buffer` line maps) used by the workaround.
- `tests/conceal_roundtrip.rs` — Extensive test coverage documenting the bug, its variants (emphasis-wrapped, strikethrough-wrapped, bare multiline wikilink), and the `\r`-only seam discovered in verification round 7.

### Acceptance Criteria

- [ ] Minimal reproduction case filed as an issue on the comrak repository, demonstrating the `sourcepos` line-number corruption for inline siblings after a multiline wikilink.
- [ ] Issue references the specific comrak version (0.54) and includes the exact input that triggers the desync.
- [ ] If comrak maintainer confirms the bug is known or already fixed in a newer version, update the dependency and remove the `subtree_has_multiline_wikilink` / `build_inlines` workaround branch.
- [ ] If the upstream fix is merged and released, remove the workaround code, update tests accordingly, and verify the full test suite passes without it.
- [ ] If the upstream will not fix (or the fix is unlikely), add a top-level module comment on `subtree_has_multiline_wikilink` linking to the upstream issue with the maintainer's response, so future readers know the workaround is intentionally permanent.

### Notes

- The workaround has a negligible performance cost: the per-line rebuild only activates when `subtree_has_multiline_wikilink` returns true, which requires a wikilink match spanning a physical line boundary — an extremely rare pattern in practice.
- The `\r`-only variant (wikilink embedding a bare `\r` without `\n`) was discovered in verification round 7 and is already handled by the current `subtree_has_multiline_wikilink` check, which looks for both `'\n'` and `'\r'`.
- The comrak frontmatter extension has a similar document-wide `sourcepos` desync (discovered in verification round 9), handled separately by `frontmatter_extension_is_safe` in `parse::block`. Both stem from the same class of issue: comrak extensions advancing internal state without updating `sourcepos` consistently.
