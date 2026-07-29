# TODO — SYNC-IDEMPOTENT catches a real reveal/scroll-pipeline lag

**Update:** a second, independent repro was found during the `make
test-fuzz` soak (no Link, no `[a](b)`, just plain prose + a scroll key):

```rune
content Hello there. This is a short prose paragraph with a few sentences in it.\n
key char:¡ ----
key down --c-
key char:¡ ----
```

Fails identically: `SYNC-IDEMPOTENT: a second sync_view() with no
intervening message changed the rendered rows (1 rows before, 2 rows
after)` — this time a ROW COUNT difference, not just cell content. Neither
repro involves any WP14-added generator surface (`F1`, `^b`/`^t`,
multicursor, `StaleConfirmTimeout`) — both use only pre-existing actions
(`Type`, `Key`, a ctrl+arrow scroll command), confirming this is a
pre-existing production defect that WP14.S1's SYNC-IDEMPOTENT fix now
reliably surfaces, not something introduced by WP14's own changes. Given
how easily both runs hit it (well under 200 generated cases each), `make
test-fuzz RC=50000` cannot exit 0 until this is fixed — recorded as a
Failure in WP14's own handoff, not papered over.

## Original repro (kept below, still valid)

**Found by:** WP14.S1's SYNC-IDEMPOTENT fix (`crates/rune-fuzz/src/driver/checks.rs::sync_idempotent_check`),
which now compares the memoized production render against BOTH a cache-bypassed
rebuild (`DocMachine::force_rebuild`) AND a genuine second, message-free
`app.sync_view()` call. The cache-bypassed comparison passes clean; the
second-real-sync comparison does not — this is a real production defect,
not a fuzz-harness false shield, and it is out of `rune-fuzz`'s scope to fix
(crates/rune-md and/or crates/rune-syntax own the reveal state machine).

**Status:** open, not fixed by WP14 (scope discipline: WP14 touches
`crates/rune-fuzz` only).

## Minimal repro

```rust
use rune_fuzz::action::Action;
use rune_fuzz::driver;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

let actions = vec![
    Action::Type("[a](b)".to_string()),
    Action::Key(KeyInput {
        code: KeyCode::Up,
        mods: Mods { ctrl: true, ..Mods::NONE },
    }),
];
let result = driver::run(driver::DOC_PATH, "", &actions);
// result.violation == Some(Violation { id: "SYNC-IDEMPOTENT", .. })
```

Equivalently: seed an empty markdown document, type `[a](b)` (a link), then
press `^Up` (`Command::ScrollLineUp`) once. `driver::run`'s own single
`app.sync_view()` call (right after `app::update` processes `^Up`) renders
the link CONCEALED (folded to its link text, `"a"`, cursor sitting on it at
byte offset 1 — which IS inside the link's reveal range, so this looks
already wrong on its own). A SECOND, completely message-free
`app.sync_view()` call — with no cursor movement, no edit, nothing else
changed — then renders the SAME link fully REVEALED (`"[a](b)"`).

## What's already ruled out

- Not memoization masking a real bug (the exact false-shield WP14.S1's own
  fix targets, CODE-REVIEW.md rune-fuzz finding 1): a cache-bypassed
  `DocMachine::force_rebuild` produces the SAME (concealed) render as the
  memoized production one. The divergence only appears against a genuine
  SECOND `sync_view()` call.
- Not a cursor-position artifact: `app.active_doc().cursors.all()` is
  identical (`position: 1, anchor: 1`) immediately before AND after the
  second `sync_view()` call — the cursor never moves between the two
  renders.
- `RevealSm::transition`/`CursorProbe::any_in`/`RevealGrant::resolve`
  (`crates/rune-syntax/src/element.rs`) are all pure functions of their
  arguments — nothing there is stateful across calls in a way that should
  explain re-deciding differently on an unchanged cursor position.

## Working hypothesis (unconfirmed — needs the owning crate's own investigation)

`Document::view()` (`crates/rune-tui/src/document.rs`) calls
`sync_content` -> `set_width` -> `sync_cursors` -> `snapshot()`, in that
order, on every call — so `sync_cursors`'s reveal-transition side effect on
`self.blocks` should already be reflected by the SAME call's `snapshot()`.
The observed one-call lag (concealed on the first post-edit sync, revealed
on the very next message-free one) suggests either:
- `commands::nav_scroll`'s `^Up`/`ScrollLineUp` handler calls `view()`
  itself for a coordinate conversion BEFORE the cursor is moved to its
  final position, and the driver's own subsequent `app.sync_view()` (called
  once per step, after `update()` returns) is a memo hit rather than a
  fresh `sync_cursors` pass because nothing marked `DocMachine::dirty` a
  second time — i.e., a scroll command's OWN internal `view()` call could be
  priming a stale cache that then survives the driver's own post-step sync;
  or
- `sync_cursors`'s dirty flag from the reveal transition doesn't propagate
  to `DocMachine::dirty` on every call the way `WrapMap`/`emit` inputs do.

Neither is confirmed; this needs someone who owns `rune-md`'s `DocMachine`/
`rune-tui`'s `commands::nav_scroll` to trace the actual call sequence with a
debugger or targeted logging, not a fuzz-side guess.

## Why this isn't papered over here

`crates/rune-fuzz` intentionally does not touch `crates/rune-md`/
`crates/rune-syntax`/`crates/rune-tui` production code (WP14's scope is the
fuzz harness itself) — the checker's job was to make this failure visible,
which it now does, deterministically, from a 2-action repro. Fixing the
underlying reveal-state lag is a separate work item.
