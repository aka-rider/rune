# Split nav.rs by command family

**Status:** open
**Priority:** low — maintenance debt. The file is 584 lines (§1.6 limit 500), 84 lines over budget.

**Symptom:** none — maintenance debt.

**Root cause:** `crates/rune-tui/src/commands/nav.rs` grew to 584 lines. The WP9.S1 Unicode word classifier (`char_class` rewrite, `is_word_forming` probe, two new Cyrillic/mixed-script regression tests) added ~52 lines on top of the pre-existing overage.

**Scope:**
- `crates/rune-tui/src/commands/nav.rs` — 584 lines, split by command family
- `crates/rune-tui/src/commands/mod.rs` — re-export surface to adjust

**Acceptance criteria:**
- No file under `crates/rune-tui/src/commands/` exceeds 500 lines after the split.
- The split groups by command family (e.g., word motions, line motions, cursor motions) rather than by arbitrary line count.
- All `pub` symbols retain their existing paths.
- `make build`, `make test`, and `make lint` pass.

**Notes:**
- The `char_class` function and `is_word_forming` probe are self-contained and could move to a sibling module if they form a natural boundary.
- Split "by command family when next touched" per the TODO entry.
