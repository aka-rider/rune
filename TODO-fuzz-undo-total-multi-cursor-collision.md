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

**Status:** open. Auto-pinned by proptest in `crates/rune-fuzz/
proptest-regressions/human_session.txt` (seed
`cc c23678f9b5d7f91498a2ed87c39a3646720dfeb1470f46c4d3395b7c071bbd65`), so
`make test-fuzz` will keep replaying and failing on it until fixed.

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
