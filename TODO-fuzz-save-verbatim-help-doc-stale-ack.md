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

**Status:** FIXED. Confirmed classification (b): production is correct here
— `Msg::SaveDone` already carries the `DocumentId` the save was actually
FOR (`save::save_cmd`'s closure captures `id`; `dispatch` forwards it
untouched to `save::handle_save_done(app, id, ...)`, doc-scoped, never
`app.active`). The dirty-close Guard's own `s`/`S` hotkey
(`banner::handle_dirty_close_key`) calls `trigger_save(app, prompt.doc,
...)` — `prompt.doc`, not `app.active` — precisely because the Guard can be
armed (via `pane::handle_quit_key`/`first_unpreserved_dirty_doc`) on a
document OTHER than whichever one is active. That's exactly what happened
here: `F1` made Help active; `Ctrl+C` (the quit chord) found the real
"hello world" document dirty and raised the Guard on IT, not Help; the
final `Cmd+S` (any `s`/`S`, modifiers unchecked by the Guard's key handler)
saved the real document correctly. Disk really did hold "hello world" —
no data was lost or misattributed in production.

The bug was in `crates/rune-fuzz` itself: `driver::step_and_check` used to
capture "the bytes this save `Cmd` was constructed with" as `prev.content`
— the Tier-1 `Snapshot`'s content field, which is always the ACTIVE
document's — rather than the actual target document's bytes. Since the
Guard's save targeted a non-active document, the driver captured the WRONG
document's bytes (Help's generated keymap table) and `SAVE-VERBATIM` then
compared disk (correctly "hello world") against that wrong capture,
misfiring a false positive. `MsgTag::SaveDone` also used to discard the
`Msg::SaveDone`'s own `id` field entirely (`..` in the destructure), so
there was no way to recover the right document short of the fix below.

Fixed in `crates/rune-fuzz`: `MsgTag::SaveDone` now carries `id:
DocumentId`; `driver::State::pending_save` snapshots every open document's
content (keyed by `DocumentId`) at the instant a save `Cmd` is constructed,
not just the active one; `discharge_pending_save` looks the correct entry
up by the ack's own `id` once it lands. `invariant/save.rs::save_verbatim`'s
doc comment (which had claimed switch-safety unconditionally) is corrected.
Regression tests
`stale_save_ack_after_help_toggle_is_attributed_to_its_own_document` and
`ordinary_same_document_save_still_clean` (`crates/rune-fuzz/tests/
tripwire.rs`) pin the exact repro below plus a same-document control (not
`#[ignore]`d, run under `cargo test --workspace`). The pinned seed in
`proptest-regressions/human_session.txt` (`cc 2ae76ac5...`) replays clean.

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
