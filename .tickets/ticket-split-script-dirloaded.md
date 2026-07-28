# Split script.rs dirloaded grammar into script_dirloaded.rs

**Status:** open
**Priority:** low — maintenance debt; the file compiles, tests pass, and the overage does not affect functionality. However, script.rs is now 634 lines against the §1.6 ceiling of 500, and it will continue to grow as new Action variants are added to the fuzz grammar.

**Symptom:** none — maintenance debt. The file violates CONSTITUTION §1.6 ("One primary type per file; decompose any file past 500 LoC") and has been deferred through multiple work packages.

**Root cause:** The WP4.S6 `dirloaded`/`dirloaded-entry` multi-line grammar added ~107 lines to an already-over-budget file (it was 501 lines before that work). The `DirEntry` name field can contain literal spaces, requiring a multi-line continuation-record shape rather than a single-line encoding — more code but strictly more correct. The growth was accepted at the time with a "split when next touched" deferral; subsequent work packages never touched script.rs, so the deferral accumulated.

**Scope:**
- `crates/rune-fuzz/src/script.rs` — 634 lines; extract the dirloaded grammar
- `crates/rune-fuzz/src/lib.rs` — add `pub mod script_dirloaded;` (or make it a child module of `script`)
- No changes to consumers — `encode`/`decode` signatures stay the same; the split is internal

**Acceptance criteria:**
- script.rs is under 500 lines after the split.
- The dirloaded grammar lives in a sibling `script_dirloaded.rs` module and includes:
  - `encode_dir_cause` and the `DirLoaded` arm of `encode_action`
  - `parse_dir_loaded` (the `Peekable`-based multi-line parser)
  - `parse_dir_entry`
  - The `DirLoaded` round-trip test cases (currently in `tests::round_trips_every_action_variant`)
- script.rs re-exports or delegates to `script_dirloaded` transparently; the public `encode`/`decode` API is unchanged.
- `std::iter::Peekable` import moves with the code that needs it (the `parse_dir_loaded` function).
- All existing tests pass; no behavior change.
- `script_dirloaded.rs` itself stays under 500 lines.
- TODO.md entry for script.rs is either removed or updated with the new line count.

**Notes:**
- The dirloaded grammar is already a self-contained unit. It depends on `DirCause` (from `rune_tui::runtime`) and `DirEntry` (from `rune_vfs`), both of which are already imported at the top of script.rs and would move with the extracted code.
- The `Peekable` type bound on `parse_dir_loaded`'s `lines` parameter is the only reason `decode` currently needs `Peekable`; after the split, script.rs may no longer need `std::iter::Peekable` at all.
- Naming convention: follow the existing pattern in the fuzz crate where sibling modules are flat files in `src/` (e.g., `script.rs`, `driver.rs`, `generate.rs`). The new file should be `script_dirloaded.rs`, not a nested `script/mod.rs` + `script/dirloaded.rs` — the latter would require restructuring the existing flat layout and adds no value for a single extraction.
