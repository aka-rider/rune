# rune-tui TODO

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
