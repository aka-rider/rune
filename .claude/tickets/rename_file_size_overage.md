# Split rename.rs to meet §1.6 file-size limit

**Status:** open
**Priority:** low — the file is 112 lines over the 500-line ceiling but is largely doc comment (~50%); no user-facing symptom, and the TODO itself notes the file should shrink naturally once per-doc recovery hydration removes the `db: None` branches.

**Symptom:** none — maintenance debt. CONSTITUTION.md §1.6 mandates one primary type per file and decomposing any file past 500 LoC. The file was over budget on day one.

**Root cause:** The rename state machine arrived at 612 lines because roughly half the file is documentation — the machine's states, the three states that deliberately do NOT exist, and the failure-atomicity reasoning that §1.4.10 turns on. The executable code is small: one state enum, `begin`, one `apply_outcome` match over four outcomes, two `Cmd` factories, and five small hooks.

**Scope:**
- `crates/rune-tui/src/rename.rs` — the state machine proper (state enum, `begin`, `apply_outcome` match, state-transition hooks).
- New file: `crates/rune-tui/src/rename_create.rs` (sibling module) — receives the two no-store `Cmd` factories (`rename_cmd` / `create_cmd`) and the draft-create route (`bind_new`).

**Acceptance criteria:**
- `rename.rs` is under 500 LoC after the split.
- `rename_create.rs` is under 500 LoC.
- The rename state machine (state enum, `begin`, `apply_outcome`, state-transition hooks) remains in `rename.rs` as the single source of truth.
- `rename_cmd`, `create_cmd`, and `bind_new` move to `rename_create.rs` with no change to call-site semantics.
- All module re-exports and `use` statements updated; `lib.rs` (or `mod.rs`) declares the new module.
- `make build`, `make test`, and `make lint` pass clean.

**Notes:**
- The TODO item explicitly says "do that when this next grows" — the split is deferred until the file actually needs it, because the per-doc recovery hydration (removing `db: None` branches) is expected to shrink the file on its own. If that shrinkage happens first and brings `rename.rs` under 500, close this ticket as resolved without action.
- The proposed split boundary is the two no-store `Cmd` factories plus the draft-create route; these are already a coherent concern (the "create-or-rename" path that does not touch the recovery store) and form a natural sibling module.
