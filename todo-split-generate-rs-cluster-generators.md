# Split generate.rs — move cluster generators to generate_clusters.rs

**Status:** open
**Priority:** low — The file exceeds the §1.6 limit by 39 lines (539 lines). No runtime impact; this is pure maintenance debt.

**Symptom:** none — maintenance debt

**Root cause:** `generate.rs` houses both the top-level arbitrary strategies (`arb_session`, `arb_cluster`, `arb_resize`, `arb_any_keycode`, `arb_mods`, `arb_dir_*`) and 11 self-contained per-cluster generator functions (`cluster_type_prose`, `cluster_navigate`, `cluster_selection`, `cluster_delete`, `cluster_undo_redo`, `cluster_markdown_write`, `cluster_save`, `cluster_clipboard`, `cluster_monkey_burst`, `cluster_async_deliver`, `cluster_chrome`). The cluster generators are leaf functions that only depend on static data and the `Action` enum, making them natural candidates for extraction.

**Scope:**
- `crates/rune-fuzz/src/generate.rs` — shrink to ~300 lines (keep static data, `arb_*` strategies, and `arb_session`)
- `crates/rune-fuzz/src/generate_clusters.rs` (new) — receive all 11 `cluster_*` functions
- `crates/rune-fuzz/src/lib.rs` — add `mod generate_clusters`

**Acceptance criteria:**
- `generate.rs` is under 500 lines after the split.
- `generate_clusters.rs` is under 500 lines.
- No changes to public API or strategy behavior — the partition is a pure refactor.
- `make build` and `make test-fuzz` pass with identical output.
- No cross-module cyclic dependency introduced.

**Notes:** The cluster functions are numbered 11 total, spanning roughly lines 375–508 in the current file. Each returns `impl Strategy<Value = Vec<Action>>` and only references static constants defined in `generate.rs` (e.g., `CONTENT_SEEDS`, `NAV_KEYS`, `PASTE_PALETTE`), so the new module will need those re-exported or the constants moved upward.
