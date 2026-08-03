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

## `app.rs` over the §1.6 500-LoC ceiling

`src/app.rs` was already 573 lines before this plan (WP1's `Document`-map
reshape, plus the doc comments recording every extraction already made —
`pane.rs`/`dispatch.rs`/`app_view.rs`/`highlight.rs` — to keep it AS SMALL
AS IT IS). Plan WP2's `quit_intent: Option<QuitIntent>` field plus
`App::is_preserved` push it further over, to ~615. Out of scope for WP2
itself (the plan's own Verification section names only `materialize_ack.rs`
and `banner.rs` as WP2's splits); tracked here rather than silently grown
past unremarked. A plausible split: `QuitIntent` plus `App::is_preserved`
are small and self-contained — the larger win is moving the long per-field
doc comments on `App` itself (many of which just narrate an already-landed
extraction) into the module doc comment or a design note, rather than
inline on the struct.
