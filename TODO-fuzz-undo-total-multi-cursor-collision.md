# TODO — `UNDO-TOTAL` fires: undo gets stuck mid-journal with multiple cursors, status `"undo failed: two edits collide on post-edit start 1"`

**Found by:** `make test-fuzz RC=50000`, the final bounded soak gate for the
`SAVE-VERBATIM` help-doc-stale-ack fix
(`TODO-fuzz-save-verbatim-help-doc-stale-ack.md`, now RESOLVED). Recorded
here per that task's own contingency clause: a DISTINCT invariant
(`UNDO-TOTAL` — undo must converge to `journal_pos == 0` within a bounded
number of steps — not `SAVE-VERBATIM`'s disk/delivered-bytes agreement) and
a completely different action shape (a paste, up-navigation with alt+sup
modifiers that mint extra cursors, typed text, copy, delete, more nav — no
`F1`/Help toggle, no save, no Guard modal anywhere) — confirmed distinct,
then NOT chased, per that task's own scope: "if it surfaces yet another
DISTINCT finding, pin+TODO it with repro and report rather than chasing."

**Status:** FIXED. Confirmed classification: a genuine multi-cursor
`ApplyInverse` construction defect, NOT a `journal_pos` accounting gap —
traced to `rune-core::undo::inverse_edits`, not `Journal`'s bookkeeping
(that stays correct throughout; the failure is `apply_inverse` itself
returning `Err`, which `commands::edit::undo` correctly refuses to commit
a position move for).

Root cause: the two live cursors (`id: 2`, `id: 3`) both sit on the SAME
line (after the earlier `Delete` press merged three typed lines into one)
and both press `⇧⌥Up` (`Command::CloneLineUp`, resolved via
`edit_lines::per_line_edits(dedupe=false)`, which deliberately lets every
cursor sharing a line clone it independently — see that function's own
doc). Both edits are pure INSERTS at the IDENTICAL pre-edit `start`
(the shared line's own start). `Buffer::apply_edits` accepts that batch —
the two `AppliedEdit`s land on DISTINCT post-edit starts once one clone's
insert shifts the other (exactly the behavior
`edit_core::coalesce_touching_edits`'s doc comment describes: "leaving
them uncoalesced does not collide" — true going FORWARD) — so the step is
recorded as a perfectly legal, undoable journal entry.

Undoing it is where this breaks: `inverse_edits` turns each pure INSERT
`AppliedEdit` into a pure DELETE `Edit` at that SAME post-edit start/end —
and because the two forward inserts were adjacent, the two inverse
deletes are exactly touching (one's end equals the other's start). Left
un-merged, THAT pair collides on its own post-edit start once shifted —
`Buffer::apply_edits` correctly refuses it as `BufferError::
DuplicateEditStart`, and `undo` correctly halts rather than corrupt the
buffer — but nothing ever produced a batch `undo` COULD apply, so the
journal wedged forever: `UNDO-TOTAL` total, not `DuplicateEditStart`'s
protection, was the thing actually broken.

Fix: `rune-core::undo::coalesce_touching_deletes` (new, shared chokepoint)
merges exactly this touching-pure-delete pair before it ever reaches
`Buffer::apply_edits`, called from `inverse_edits` right after building the
raw inverse batch. `rune-tui`'s own `edit_core::coalesce_touching_edits`
(the forward-side pure-delete merge this defect's sibling case already
required) now delegates to the same function instead of duplicating the
merge condition. Both existing protections are intact and re-verified by
their own still-green tests: `DuplicateEditStart` still refuses any batch
that collides AFTER this merge (a corrupted/adversarial persisted journal
row, say), and the clone-line multi-cursor-survival guarantee is
untouched (this fix never touches forward per-cursor construction or
cursor restoration — undo restores cursors from the step's own recorded
`cursors_before`, never re-derived from the inverse batch).

Regression coverage: `rune-core::undo::tests::
apply_inverse_undoes_a_two_cursor_same_line_clone` pins the exact
colliding shape at the `Buffer`/`AppliedEdit` level; the fuzzer's checked-in
`crates/rune-fuzz/repros/undo-total-clone-line-01.rune` pins the original
session shape (asserted clean by `tests/replay.rs`, which runs on every
`cargo test --workspace`); the original pinned seed in `crates/rune-fuzz/
proptest-regressions/human_session.txt` (`cc
c23678f9b5d7f91498a2ed87c39a3646720dfeb1470f46c4d3395b7c071bbd65`) now
replays clean under `make test-fuzz`.

## Minimal repro (frozen artifact)

`crates/rune-fuzz/artifacts/undo-total-6acc776b/` (`report.txt` +
`script.rune`, gitignored — regenerate with `make test-fuzz RC=50000` if
they no longer exist locally):

```
content <empty>
paste "\n\n\n"
key up (alt, sup)
key up (alt, sup)
type "hello world"
key char:c (sup, i.e. Cmd+C)
key delete
key up (shift, alt)
key char:¡
```

**The violation:** `snapshot.status` reads `"undo failed: two edits collide
on post-edit start 1"` and `journal_pos` gets stuck at 14 of 15 — undo never
converges back to 0 within `UNDO-TOTAL`'s bound. `snapshot.cursors` shows
TWO live cursors (`id: 2` at position 35, `id: 3` at position 69) over a
buffer that's `"hello world"` repeated three times per line across three
lines — the `alt`-modified `Up` keys almost certainly minted extra cursors
(`Command::AddCursorAbove`/multi-cursor nav), and the subsequent typing/
copy/delete were then applied at each cursor. The status message
`"two edits collide on post-edit start 1"` names the exact failure mode:
whatever inverse-apply step undo is walking back through hit two recorded
edits whose post-edit ranges overlap at byte offset 1, and the journal's
replay machinery refused to proceed rather than corrupt the buffer (a safe
halt, not a crash) — but it never recovers from that halt, so `UNDO-TOTAL`'s
progress bound trips.

This is very likely a multi-cursor journal/undo defect (production code,
not a fuzz-harness modeling gap) — `Journal`'s edit-batch coalescing or
`ApplyInverse`'s per-cursor replay may be constructing an inverse batch
whose per-cursor ranges weren't correctly re-based against each other once
one of the batch's own edits already shifted a later cursor's offsets. But
this has NOT been traced — whoever picks this up should start at
`Journal`'s multi-cursor edit-batch commit path and `commands::edit::undo`'s
replay, confirming whether the collision is a genuine overlapping-range bug
in how a multi-cursor batch's inverse is constructed, or a `journal_pos`
bookkeeping bug that miscounts steps for a multi-cursor batch (which would
make this a harness-side accounting gap instead) — do not assume which one
it is without tracing the actual multi-cursor edit-batch construction and
its inverse.
