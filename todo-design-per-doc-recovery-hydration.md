# Design per-doc async hydration for explorer-opened documents

**Status:** open
**Priority:** medium — a real, bounded data-safety gap: Explorer-opened documents have no recovery journal, so a crash while dirty loses that tab's unsaved edits with no recovery-store anchor.

**Symptom:** When the user opens a document via the Explorer (not the bootstrap document passed on the command line), that document has `db: None` — no per-doc recovery journal. If the app crashes while that document is dirty, the unsaved edits are lost with nothing in the recovery store to restore from.

**Root cause:** `workspace::open_path` (`crates/rune-tui/src/workspace.rs`) opens a Document selected in the Explorer with `db: None`. The bootstrap document `rune-cli::main` hydrates through `rune-db::load` before the TUI starts, but Explorer-opened documents skip this step entirely. Phase 1 accepted the same risk for every document before the recovery store existed; the MVP deferred it so WP4 stays an architecture-validation MVP.

**Scope:**
- `crates/rune-tui/src/workspace.rs` — `open_path` is the caller that needs to trigger hydration
- `crates/rune-db/src/store.rs` — `Store::load` is the hydration primitive
- `crates/rune-tui/src/document.rs` — the `db: None` field that needs to be populated
- `crates/rune-tui/src/app.rs` — may need to handle hydration ack messages

**Acceptance criteria:**
- A design document (or implementation plan) describing the async hydration flow: `Store::load` per `open_path`, ack-driven, mirroring the bootstrap path.
- The design specifies how the Explorer open command triggers the hydration, how the ack is delivered, and what happens if hydration fails.
- The design ensures no blocking on the render path (§5.3 Non-Blocking Update).
- The design is reviewed and approved before implementation begins.

**Notes:**
- Deliberately deferred so WP4 stays an architecture-validation MVP rather than combining routing and async hydration under one deadline.
- CONSTITUTION §1.4.1, §1.4.2 are relevant for the materialize path; the hydration design must not violate the atomic publish contract.
- The bootstrap path (`rune-cli::main` → `rune-db::load`) is the reference implementation for the hydration shape.
