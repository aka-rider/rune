# TODO — Refactor Ledger

Reinstated 2026-08-07 from a three-audit sweep of the codebase. This is an
inventory of known violations of `CONSTITUTION.md`'s governing rules and of
non-idiomatic implementations with better alternatives available; it seeds a
future refactor plan. New code must not repeat these patterns even where
legacy code still does. Fixes land with tests per the constitution, and an
entry is deleted in the same commit that fixes it.

## Bug-hunt sweep (2026-08-26)

Product of a parallel multi-crate bug hunt (eight review passes, one per
subsystem plus a cross-cutting security sweep). These are *defects*, not
style — each carries a concrete failure scenario and a confidence tag.
`confirmed (executed)` means the reviewer built a repro and ran it;
`confirmed` means fully traced in source; `plausible` means the mechanism
is certain but one link (usually a live user sequence) was not reproduced.
Entries are ordered by severity. Fixes land with a regression test per the
constitution and the entry is deleted in the same commit.

### DATA-LOSS

### CRASH / DoS

### SECURITY

### CORRECTNESS

### MINOR

## Architecture

## Mechanical

### O(file) per keystroke is the deliberate design ceiling
- **Where**: `crates/rune-core/src/buffer/mod.rs`, `crates/rune-core/src/buffer/lineindex.rs`, `crates/rune-tui/src/commands/edit_core.rs`, `crates/rune-tui/src/materialize_ack.rs`; perf-guarded by `crates/rune-tui/tests/perf_guard.rs:92` (`keystroke_view_cost_under_budget_on_a_5k_line_code_document`)
- **Wrong**: full content copy + full line-index clone + full memcmp + journal clones per edit batch; does not scale past the guard fixture's size.
- **Instead**: a rope with the same value-semantics facade, if the ceiling is ever hit in practice.
- **Done when**: not currently actionable — record only; revisit if the perf guard's fixture size stops matching real documents.

## Mutation testing (2026-08-27)

cargo-mutants is integrated: config in `.cargo/mutants.toml`, `make mutants`
(`PKG=<crate>` scopes, `J=` jobs, `MUTANTS_ARGS=` passthrough). Seven crates
were driven to zero unjustified missed mutants; every exclusion in
`.cargo/mutants.toml` carries its proof in the commit that added it.

### Covered (missed mutants at pass end / total generated)
- rune-image 0/143 · rune-nav 0/56 · rune-syntax 0/397 · rune-core 0/309 ·
  rune-ts 0/103 · rune-cli 1/99 · rune-md 2/869 (final verify pass pending
  at ledger time; residuals below)

### Known residual misses (documented, deliberately not excluded)
- `crates/rune-cli/src/main.rs` `AppGuard::drop -> ()` — proving the drop's
  effect needs an interactive session or a sleep-free sync point that does
  not exist; a future PTY harness can claim it.
- `crates/rune-md/src/invariant.rs` `assert_no_duplicate_content{,_at} -> ()`
  — killing them needs a document that violates the invariant; 100k+ fuzz
  inputs found none because the pipeline appears correct. A regression should
  still be able to surface them.

### Not yet mutation-tested
- rune-merge, rune-vfs, rune-db — deferred: they carried in-flight DATA-LOSS
  fixes from the 2026-08-26 sweep during this pass; run them once that work
  settles. rune-vfs/rune-db are the prime-directive crates — highest value
  next.
- rune-tui (51k LOC) and rune-fuzz (the harness itself) — out of scope this
  pass by size and role.

### Caveats and follow-ups
- This pass ran on Linux. `#[cfg(target_os = "macos")]` blocks — including
  the exchange/fsync branches in `crates/rune-vfs/src/disk.rs` and
  `crates/rune-db/src/session.rs` — were never compiled, so no mutants were
  generated for them. A macOS run is needed before "0 missed" means anything
  for the durability paths.
- Some `exclude_re` entries pin `file:line` (noted in their commits); they
  need re-justification when those lines move.
- Cheap CI follow-up: a PR-scoped `cargo mutants --in-diff` job (the book's
  pr-diff workflow) once a macOS runner budget exists.
- The `--skip` test-name filters matter: `dependency_guard` (rune-nav) and
  the source-grep gate `self_state_assignment_is_scoped_to_the_two_transition_writers`
  (rune-md) read the tree, not the mutant, and must not run under mutants —
  the rune-md gate false-catches mutants if left in.
