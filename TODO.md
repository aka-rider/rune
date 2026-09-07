# TODO

## CLIP-OSC52 fails on cut inside reading view (found by `make test-fuzz`)

`make test-fuzz` (the `human_session` fuzzer) deterministically reproduces a `CLIP-OSC52`
violation, unrelated to the `SCROLL-IN-DOC` invariant work that surfaced it (that work only
added a new, independent check; this failure trips an existing, older invariant). Minimal
repro script (see `crates/rune-fuzz/artifacts/proptest-regressions/human_session.txt`):

```
type hello world
diverge-disk
deliver-db-all
key char:m ctrl        (merge chord)
deliver-db-all
key char:y shift+sup    (redo? — see MergeState)
key char:m ctrl
deliver
key right shift          (extend selection)
key char:P ctrl           (toggle reading view — ReadOnly::Reading)
key char:x sup             (cmd+x, cut)
```

Panic: `CLIP-OSC52: no OSC 52 raw chunk decoded to the selected text "f"; raw chunks emitted: 0`

Cutting (cmd+x) a selection while the document is in reading view (read-only) emits no OSC 52
clipboard chunk at all — not even a copy-only fallback. Needs a real fix: either cut in a
read-only document should still copy the selection to the clipboard (just skip the delete), or
the fuzzer's own `CLIP-OSC52` invariant needs a documented read-only carve-out if that refusal
is intentional product behavior. Not investigated further — out of scope for the viewport-clamp
fix this TODO was filed alongside.

## Named drafts left behind on discard

A launch positional that does not exist on disk yet is opened as a named scratch row
(`bootstrap_new_file` in crates/rune-cli/src/db_bootstrap.rs, `create_named_scratch`). Closing
it with ^W and answering discard leaves that row in the recovery store, so it is offered back
on the next launch of the same path. Untitled drafts now go through `Store::forget_scratch` on
close; named drafts should too once it is decided whether a discarded named draft must still be
offered back.

## Double adoption of a dead session's draft

Adopting a recovered scratch row at launch claims nothing in `session_documents` until the first
journal write (see `adopt_scratch_doc` in crates/rune-tui/src/db_ack.rs and `reconstruct_scratch`
in crates/rune-db/src/scratch.rs). Two launches racing before either journals can both adopt the
same row. Now that `forget_scratch` can delete a row and SQLite reuses the freed rowid
(`documents.id` has no AUTOINCREMENT), the loser can journal onto a rowid that was re-minted for
a different draft. Fix candidate: claim the row in `session_documents` at adoption time, before
binding.

## Chord labels hard-coded in user-facing text

Several messages and footer rows spell a chord by hand instead of reading it from the binding
table through `global::label_for` / `hint_for`: `^M` in `materialize_ack/reactions.rs` ("^M to
merge") and `db_ack.rs` ("[^M]erge"), and `^K` in `footer_modes.rs`. The rename hint in
`reactions.rs` rotted exactly this way when rename moved from `^R` to `F2` and was fixed in that
change, and the find panel's "finish the merge first" refusal now reads its chord from the table;
the rest will rot the same way the day their chord moves. Fix: one chokepoint that formats a
message's chord from its `GlobalCommand` (or pane command), and tests that assert the label comes
from the table.

## `replace_history` and `command_history` tables were added in place

`CONSTITUTION.md` names a nullable column as the one schema change that may land under the same
`rune-vN.db` filename; a new table is supposed to ship as a new file. `command_history` set the
precedent of appending a `CREATE TABLE IF NOT EXISTS` to `SCHEMA` without a `SCHEMA_VERSION`
bump, and `replace_history` (the Replace field's own history, same shape as `search_history`)
followed it, because a bump would strand every unsaved document's recovery journal in the old
file for the sake of a history table nobody depends on. Decide whether the rule or the precedent
wins: either the constitution admits "a new table with no foreign keys" as an additive change,
or both tables move to `rune-v3.db` together with a migration that carries the journals across.

## `^⇧P` is swallowed silently while the find panel has focus

`commands::reading::toggle` returns without a word unless the editor or the palette owns focus,
and `GlobalCommand::ToggleReadOnly` is `BarPolicy::LeaveOpen`, so pressing the reading-view chord
with the find panel focused consumes the key and changes nothing. Either the toggle should apply
to the active document regardless of which overlay holds the keyboard (the panel already works on
a read-only document), or it should say why it refused.

## The fuzzer never sees project search results

`driver/step_exec.rs` drops every `CmdKind::ProjectIndex` and `CmdKind::ProjectQuery` the update
loop spawns, and the session fixture seeds one file with no project root. So `cluster_find`'s
Project-scope steps (open the top hit, `⇧⏎` replace-all across files) always run against an empty
result list, and `FIND-REPLACE-NO-DISK` / `FIND-WALK-DRAINS` hold trivially. Fix candidates: seed a
small multi-file `Mem` project under a root in `Session::open` and run the index/query cmds inline
the way `ReadFile` replies are already driven, so a real walk (tab limit, queued loads, evicted
origin) is reachable from generated sessions.
