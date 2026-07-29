# TODO — `SAVE-VERBATIM` fires: a stale save ack lands against the Help document instead of the doc it actually saved

**Found by:** `make test-fuzz RC=50000`, the final full soak gate for the
`TABLE-ROW-WIDTH` extra-columns fix (`TODO-fuzz-table-row-width-extra-
columns.md`, now RESOLVED). Recorded here per that task's own contingency
clause: a DISTINCT invariant (`SAVE-VERBATIM` — the delivered `SaveDone`
bytes must byte-equal what's actually on disk for the document the save
was FOR — not `TABLE-ROW-WIDTH`'s per-row summed-width agreement) and a
different action shape (typing, opening the generated Help document via
`F1`, then saving — no table involved at all) — confirmed distinct, then
NOT chased, per that task's own scope: "if it surfaces yet another
DISTINCT finding, pin+TODO it with repro and report rather than chasing."

**Status:** open. Auto-pinned by proptest in `crates/rune-fuzz/
proptest-regressions/human_session.txt` (seed
`cc 2ae76ac56c2d03560131a47bf630317671b738c7d2c77ea04eab1dddc5b9c3d2`), so
`make test-fuzz` will keep replaying and failing on it until fixed.

## Minimal repro (frozen artifact)

`crates/rune-fuzz/artifacts/save-verbatim-416c4e37/` (`report.txt` +
`script.rune`, gitignored — regenerate with `make test-fuzz RC=50000` if
they no longer exist locally):

```
content <empty>
type "hello world"
key f1
key char:a
key char:a
key char:c (ctrl)
stale-confirm-timeout 4294967295
key char:s (sup, i.e. Cmd+S)
```

Sequence: an empty document gets `"hello world"` typed into it, then `F1`
opens the generated (virtual, read-only, "never dirty" per this repo's own
vocabulary) Help document — switching `App::active` to the Help
`DocumentId`. Two more keys land (`a`, `a` — presumably no-ops or nav on
the read-only Help doc), then `Ctrl+C` (a dirty-close guard modal,
mirroring the same pattern the `TABLE-ROW-WIDTH` fix's own seed hit), a
stale-confirm timeout, then `Cmd+S` (save).

**The violation:** the `SaveDone { version: 12, ok: true }` message that
arrives is treated as confirming the ORIGINAL document's ("hello world")
save, and `ctx.disk` does hold `"hello world"` — the save the user
actually asked for DID land correctly on disk — but the snapshot's
`active` document at the point the ack is checked is `DocumentId(2)` (the
Help document, `snapshot.content` is the whole generated keymap-table
text), and the checker compares the ACTIVE document's content against
disk. So this may not even be a real data-loss bug: it may be the
`SAVE-VERBATIM` checker itself comparing the wrong document's content
against disk after a mid-flight active-document switch, rather than the
save/ack plumbing genuinely misattributing which document a `SaveDone`
belongs to. Distinguishing those two shapes — (a) a save's ack genuinely
gets attached to the wrong `DocumentId`, a real correctness bug, vs. (b)
the fuzz invariant checker reads `snapshot.active`'s content instead of
tracking which document the in-flight save was actually FOR — is exactly
where whoever picks this up should start; do not assume which one it is
without tracing `Cmd+S`'s own document-id capture through to where
`SAVE-VERBATIM` reads `ctx.disk`/`snapshot.content` (`crates/rune-fuzz/
src/invariant` and `driver/checks.rs`).
