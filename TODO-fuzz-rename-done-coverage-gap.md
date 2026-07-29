# TODO — Action::RenameDone (CODE-REVIEW.md rune-fuzz finding 8) not implemented

**Status:** deliberately deferred, not a silent skip.

WP14.S3 directs adding `Action::RenameDone { generation, ok }` on the
`DirLoaded` synthesized-reply shape (including deliberately-stale
generations), to fuzz `rune-tui`'s rename-completion path (the generation-
staleness guard, `GuardKind::RenameCollision`, and the whole §1.4.10-bearing
collision path) — currently reachable only through the ARMING half, never
the completion half.

## Why this wasn't implemented in WP14

`Msg::RenameDone`'s payload is `{ generation: u32, result: Result<rune_db::
RenameOutcome, String> }` (`crates/rune-tui/src/runtime/mod.rs`) — even the
DB-free "no-store route" variant (`RenameOutcome::Renamed { to: PathBuf }`,
constructed by `rename::rename_cmd`, `crates/rune-tui/src/rename.rs`) is a
type OWNED BY `rune-db`. To construct one, `crates/rune-fuzz` would need to
name `rune_db::RenameOutcome` directly, which requires listing `rune-db` as
an explicit Cargo dependency — Rust's extern-prelude rules only expose a
crate's own DIRECT dependencies, not transitive ones, so there is no way to
spell this type without that addition.

This crate's own module docs (`lib.rs`) state the boundary explicitly:
"This crate deliberately does NOT depend on `rune-db` (plan WP7.S10) — no
journal-coalescing/recovery-store invariant ... can be expressed here."
Adding `rune-db` as a dependency just to name one enum variant would breach
that documented, deliberate architectural boundary — a decision this WP was
not chartered to relitigate (WP14's scope is `crates/rune-fuzz` itself, not
cross-crate dependency policy).

The alternative — adding a small `rune-tui` production helper (e.g.
`rename::synthetic_rename_done(generation, ok) -> Msg`) that hides the
`rune_db` type behind a boolean, so `rune-fuzz` never has to name it —
would be a production-code change, also out of WP14's scope ("Do not
modify production save/render logic").

## What this leaves uncovered

Same as CODE-REVIEW.md's own description: the rename-completion path
(`handle_rename_done`/the store-backed `handle_rename_ack` equivalent),
`GuardKind::RenameCollision`, and the §1.4.10 displaced-bytes capture on a
collision are still unexercised by the session fuzzer. The ARMING half
(triggering a rename via the title field) is already fuzzed; only the
async completion reply is missing.

## Suggested next step

Either:
(a) relitigate the "no rune-db dependency" boundary for `rune-fuzz`
    (a plan-level decision, not a WP14 unilateral call), or
(b) add a narrow, `rune_db`-free construction seam in `rune-tui` (a
    production change, needs its own WP/review) that lets a fuzz driver
    synthesize a `Msg::RenameDone` from just `(generation, ok: bool)`
    without naming `RenameOutcome` itself.
