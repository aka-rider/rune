# Split explorer.rs — move binding table and key handler to sibling module

**Status:** open
**Priority:** low — Over the §1.6 limit by 53 lines (553 lines). The overage grew 6 net lines from the WP6.S1 `Binding<C>` shape change. The explorer is fully functional; this is maintenance debt.

**Symptom:** none — maintenance debt

**Root cause:** `explorer.rs` bundles the `Explorer` struct, `ExplorerCommand` enum, navigation helpers (`move_selection`, `ensure_visible`, `visible_rows`), action functions (`open_selected`, `go_to_parent`, `request_dir`), the `EXPLORER_BINDINGS` table, the `handle_key` dispatcher, and the `draw` function. The bindings table and key handler form a self-contained unit that maps keys to commands, mirroring the pattern already established by the `keymap.rs` → `binding.rs` / `global.rs` split.

**Scope:**
- `crates/rune-tui/src/explorer.rs` — retain `Explorer`, `ExplorerCommand`, navigation/action functions, `initial_root`, `draw`, `truncate_root`; target ~380 lines
- `crates/rune-tui/src/explorer_bindings.rs` (new) — receive `EXPLORER_BINDINGS` and `handle_key`
- `crates/rune-tui/src/lib.rs` — add `mod explorer_bindings`

**Acceptance criteria:**
- `explorer.rs` is under 500 lines after the split.
- `explorer_bindings.rs` is under 500 lines.
- The `handle_key` function signature remains unchanged with identical behavior.
- `EXPLORER_BINDINGS` retains its current type and contents.
- `make build` and `make test` pass with no visual or behavioral change.
- No cyclic dependency between `explorer` and `explorer_bindings`.

**Notes:** The `handle_key` function currently lives around line 143 and the `EXPLORER_BINDINGS` table sits just above it. The handler references `ExplorerCommand` variants, so the new module will need to import from the `explorer` module. This mirrors the `global.rs` → `binding.rs` relationship where the bindings module imports the command type from the core module.
