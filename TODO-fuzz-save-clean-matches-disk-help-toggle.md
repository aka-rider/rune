# TODO — SAVE-CLEAN-MATCHES-DISK fires on F1 while the real document has

unsaved edits after the switch (help toggle)

**Found by:** `make test-fuzz RC=200` while verifying the fix for
`SAVE-INFLIGHT-SM`'s active-document-switch gate (`fix-save-inflight-help`
branch). That fix stopped the driver from aborting early at the `F1` step,
which let the SAME underlying script run one action further and trip a
SECOND, distinct invariant. Not chased per the parent task's instructions
("if the soak surfaces yet another distinct violation, do not chase it").

**Status:** open, out of scope for `fix-save-inflight-help` (that branch's
scope is `SAVE-INFLIGHT-SM` only).

## Minimal repro

```rust
use rune_fuzz::action::Action;
use rune_fuzz::driver;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

let actions = vec![
    Action::Type("hello world".to_string()),
    Action::Key(KeyInput { code: KeyCode::Char('s'), mods: Mods { sup: true, ..Mods::NONE } }),
    Action::Key(KeyInput { code: KeyCode::Char('A'), mods: Mods::NONE }),
    Action::Key(KeyInput { code: KeyCode::F1, mods: Mods::NONE }),
    Action::Key(KeyInput { code: KeyCode::Char('A'), mods: Mods::NONE }),
];
let result = driver::run("/fuzz/doc.md", "", &actions);
// result.violation == Some(Violation {
//     id: "SAVE-CLEAN-MATCHES-DISK",
//     message: "document reports clean but disk does not match content:
//                disk=Some(\"hello world\") content=\"# Help\n\n## Global\n\n...\"",
// })
```

Already recorded in `crates/rune-fuzz/proptest-regressions/human_session.txt`
(seed `cc 9d4f5b0d...`, last line) — proptest will re-run it on every future
`make test-fuzz`.

## Working hypothesis (unconfirmed, same class as SAVE-INFLIGHT-SM's fix)

`save_clean_matches_disk` (`crates/rune-fuzz/src/invariant/save.rs`) is a
single-`Snapshot` check: once `!next.is_dirty` with a save delivered and
none pending, it demands `ctx.disk` byte-equal `next.content`. Both
`next.is_dirty` and `next.content` are doc-scoped to whichever document is
CURRENTLY ACTIVE (`Snapshot::capture` reads them off `app.active_doc()`),
but `ctx.disk` is `mem.read()` against the path of the document the save
was actually issued against — it does not follow the active-document
switch at all.

`F1` (help toggle) swaps `app.active` to the virtual, always-clean,
never-backed-by-a-file Help document. On the next step, `next.is_dirty` is
trivially `false` (Help is never dirty) and `next.content` is the Help
document's synthetic markdown — neither has anything to do with
`ctx.disk`, which still holds whatever the real document last had written
to it. The mismatch the checker reports is not a real durability defect;
it is the checker comparing two unrelated documents' facts.

This checker takes `next: &Snapshot` only (no `prev`), so the existing
`prev.active == next.active` gate pattern (`VERSION-MONOTONE`, `REDO-CLEAR`,
and this branch's `SAVE-INFLIGHT-SM` fix) doesn't transplant directly —
whoever picks this up needs either: (a) `StepCtx` to also carry `prev`'s
active id (or the driver to pass both snapshots to this checker), or (b) a
`next.read_only` (or equivalent "is this the virtual Help document") gate,
confirming first whether the real, non-virtual document could ALSO be
active with a stale disk read for some other legitimate reason before
picking the gate's shape.

Needs someone who owns `rune-fuzz`'s invariant checkers to decide the
precise gate and add the doc-scoped regression test alongside
`save_inflight_sm`'s (`crates/rune-fuzz/tests/invariants/protocol.rs`
pattern; this one lives in `save_disk.rs` in the same test dir).
