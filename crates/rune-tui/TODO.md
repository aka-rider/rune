# rune-tui TODO

## No recovery journal for the default untitled document

`rune` with no file argument now opens via `App::new_untitled` (`src/app.rs`)
instead of exiting `EX_USAGE` — the default entry point since this launch
mode landed. That document opens with `db: None`: `rune-cli::main`'s
`bootstrap_db` only ever runs for a *file argument* that already exists on
disk (`rune_db::load` requires the target to be present — see
`bootstrap_db`'s call site in `crates/rune-cli/src/main.rs`), and a brand-new
untitled document has no `documents` row for it to hydrate in the first
place.

Concretely: until the user names it (^R) and the first materialize commits,
this document has no journal, no snapshots, and no crash recovery — a
process crash or `kill -9` loses whatever was typed, in direct tension with
CONSTITUTION's Prime Directive ("protect the user's words"). This gap
pre-dates the default launch mode (an Explorer-opened document has the same
`db: None` shape, Assumption A1, `workspace::open_path`), but it is now
reachable by simply running `rune` with no arguments at all — previously
that was `EX_USAGE` and no document was ever open long enough to matter.

Closing it needs a `rune-db` binding for a document that has no path yet —
i.e. a `documents` row that isn't keyed off an on-disk file — which WP4
deliberately left out of scope (`document.rs`'s module doc: "WP4
deliberately left 'create a scratch/untitled document' out of scope").
Building that binding is out of scope for the untitled-document-startup fix;
tracked here instead of silently skipped.

## `rename.rs` over the §1.6 500-LoC ceiling

`src/rename.rs` is ~612 lines, over the CONSTITUTION §1.6 budget
(checklist line 278). Pre-existing (the rename state machine — `Ticket`,
`RenameState`, `begin`/`apply_outcome`/`bind_to`/`replace_confirmed`/
`bind_new` and their `Cmd` builders — was already this large before the
untitled-document-startup change). Splitting it is out of scope for that
change; tracked here instead of silently skipped. A plausible split: the
`Collision`/`Capturing`/`[R]eplace` machinery (`replace_allowed` through
`replace_confirmed`) into its own `rename_replace.rs`, leaving `rename.rs`
with `begin`/`apply_outcome`/`bind_to`/`bind_new` and the plain no-store
`Cmd` builders.
