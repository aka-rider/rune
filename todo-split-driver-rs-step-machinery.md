# Split driver.rs — hoist step machinery to driver_step.rs

**Status:** open
**Priority:** low — Over the §1.6 limit by 50 lines (550 lines). The growth came from the `UNDO-TOTAL` harness fix that added the `restore_editor_focus` helper. The file is functional; this is maintenance debt.

**Symptom:** none — maintenance debt

**Root cause:** `driver.rs` currently packs the session runner (`run`), the step loop (`step_and_check`), panic catching (`run_update_catching_panic`, `downcast_panic`), the step state machine (`State`, `Outcome`), and several end-of-session checks (`sync_idempotent_check`, `wrap_rt_check`, `build_rows_or_empty`). The `run` entry point and the session-drive logic (`key_step`, `restore_editor_focus`, `discharge_pending_save`) are the file's primary concern; the rest is reusable step infrastructure.

**Scope:**
- `crates/rune-fuzz/src/driver.rs` — retain `run`, `key_step`, `restore_editor_focus`, `discharge_pending_save`, and end-of-session checks; target ~280 lines
- `crates/rune-fuzz/src/driver_step.rs` (new) — receive `State`, `Outcome`, `step_and_check`, `run_update_catching_panic`, `downcast_panic`, `should_sample`, `build_rows_or_empty`
- `crates/rune-fuzz/src/lib.rs` — add `mod driver_step`

**Acceptance criteria:**
- `driver.rs` is under 500 lines after the split.
- `driver_step.rs` is under 500 lines.
- The `RunResult` public type and `run()` public function remain in `driver.rs` with unchanged signatures.
- `make build` and `make test-fuzz` pass with identical output.
- No change to violation detection behavior or snapshot sampling.

**Notes:** `State` and `Outcome` are currently private structs. After the split, `driver_step.rs` will own them and expose them to `driver.rs` via `use crate::driver_step::{State, Outcome}`. The `RunResult` struct should stay in `driver.rs` since it is the crate's public return type.
