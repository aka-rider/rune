# TODO — `SYNC-IDEMPOTENT` fires on a table document after `^Down` (scroll) then two unbound chars

**Found by:** `make test-fuzz RC=50000`, the same soak run as
`TODO-fuzz-undo-total-dirty-close-discard.md`. Recorded per the same
contingency clause — confirmed distinct from the active-document-switch
class this task was scoped to (single document throughout, no `F1`, no
switch of any kind) and from both already-`RESOLVED` `TABLE-ROW-WIDTH`
entries in `TODO.md` (a different invariant: row-width vs. render/scroll
idempotence) — then NOT chased, per scope.

**Status:** open, out of scope for `fix-invariant-doc-switch`. Pinned in
`crates/rune-fuzz/proptest-regressions/human_session.txt`, so `make
test-fuzz` will keep replaying and failing on it until fixed.

## Minimal repro

```rust
use rune_fuzz::action::Action;
use rune_fuzz::driver;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

const DOC: &str = "# Doc\n\n| Name | Age |\n| :--- | ---: |\n| Alice | 30 |\n| Bob | 25 |\n\ntail\n";

let actions = vec![
    Action::Key(KeyInput {
        code: KeyCode::Down,
        mods: Mods { ctrl: true, ..Mods::NONE },
    }),
    Action::Key(KeyInput { code: KeyCode::Char('¡'), mods: Mods::NONE }),
    Action::Key(KeyInput { code: KeyCode::Char('¡'), mods: Mods::NONE }),
];
let result = driver::run("/fuzz/doc.md", DOC, &actions);
// result.violation == Some(Violation {
//     id: "SYNC-IDEMPOTENT",
//     message: "a second sync_view() with no intervening message changed \
//                the rendered rows (8 rows before, 9 rows after)",
// })
```

Also pinned in `crates/rune-fuzz/proptest-regressions/human_session.txt`
(seed `cc 5ba2b20c...`) — proptest replays it on every future `make
test-fuzz`. Frozen artifact: `crates/rune-fuzz/artifacts/sync-idempotent-96525109/report.txt`.

## What's known (not investigated further, per scope)

The frozen snapshot shows the violation fires on step 1 itself — the very
first key (`^Down`, `Command::ScrollLineDown`) already leaves
`sync_idempotent_check`'s cache-bypassed rebuild disagreeing with a second,
message-free `app.sync_view()` (8 rows before, 9 after) on a document
containing a boxed table. `content`/`version`/`is_dirty` are unchanged
(`journal_pos`/`journal_len` both `0`, `is_dirty: false`) — this is a pure
render/scroll-pipeline disagreement, not an edit. Given the table-rendering
surface has a recent history of two now-`RESOLVED` `TABLE-ROW-WIDTH`
defects (`TODO.md`, both traced to the Grid column-width/grapheme-run
measurement mismatch), this is plausibly a THIRD table-rendering defect —
but a different invariant (`SYNC-IDEMPOTENT`'s scroll/row-count
idempotence, not `TABLE-ROW-WIDTH`'s per-row width agreement) — and that
plausibility has not been verified. Whoever picks this up should start from
`render::overlay`/`Viewport::reconcile`'s interaction with a boxed table's
synthesised border rows under a scroll command, per `sync_idempotent_check`
(`crates/rune-fuzz/src/driver/checks.rs`)'s own module docs on what each
half of `SYNC-IDEMPOTENT` is supposed to prove.
