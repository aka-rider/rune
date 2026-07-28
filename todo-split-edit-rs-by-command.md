# Split rune-tui edit.rs by command family to restore §1.6 file-size compliance

**Status:** open
**Priority:** low — the file compiles, functions correctly, and the overage is entirely structural debt. No user-facing symptom. Contribute to bus factor risk and slow incremental compile times for the commands module, but not on any active development path.

**Symptom:** none — maintenance debt.

**Root cause:** `crates/rune-tui/src/commands/edit.rs` grew to 828 lines when WP4's `retract_space` feature (bespoke single-cursor, selection-safe space-retraction edit) and its six unit tests were added. The file had already been at 695 lines, so the split was deferred at that point and the overage accumulated. CONSTITUTION.md §1.6 caps files at 500 lines; a large share of the current bloat are module-local unit tests that belong in an integration test crate.

**Scope:**
- `crates/rune-tui/src/commands/edit.rs` — primary target; split into two or more files grouped by command family.
- `crates/rune-tui/src/commands/mod.rs` — re-export surface to adjust.
- New integration test module (location TBD under `crates/rune-tui/tests/` or an existing test harness) — destination for the six `retract_space` tests plus any other module-local tests that only call `pub` functions.

**Acceptance criteria:**
- No file under `crates/rune-tui/src/commands/` exceeds 500 lines after the split.
- All public symbols previously exported from `edit.rs` remain accessible at their existing paths (no downstream breakage).
- The six `retract_space` unit tests (and any other moved tests) pass as integration tests from their new location.
- `make build`, `make test`, and `make lint` all pass green.
- The TODO entry in the rust port §1.6 overages section is removed or updated to reference this ticket as resolved.

**Notes:**
- The TODO explicitly says "split by command family when next touched," so this ticket can absorb any adjacent work on the edit commands module without extra coordination cost.
- When deciding split boundaries, group by edit command family (e.g., insert/delete/retract vs. structural edits) rather than by test vs. implementation — tests stay with their code unless they only exercise `pub` surfaces, in which case they move to integration tests.
- Check whether other files in the same directory also approach the 500-line limit; if so, note them in a follow-up ticket rather than deflating this one further.
