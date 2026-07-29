# TODO — `UNDO-TOTAL` fires when a quit-chord's dirty-close Guard discards the seeded document while a different document is displayed

**Found by:** `make test-fuzz RC=50000`, run while soaking the
`SAVE-CLEAN-MATCHES-DISK`/help-toggle fix (`fix-invariant-doc-switch`
branch). Recorded here per that task's own contingency clause ("if the soak
surfaces yet another distinct violation that is NOT the active-document-
switch class, record it and finish — do not chase it"): traced far enough to
confirm this is a genuinely DIFFERENT root cause, not a fourth instance of
that class, then stopped.

**Status:** FIXED on `fix-undo-total-dirty-close-discard`. Confirmed by
tracing `pane::handle_quit_key`/`first_unpreserved_dirty_doc` and
`banner::handle_dirty_close_key`/`workspace::close_now`: the Guard firing
for a non-active dirty document, and the discard permanently closing it, are
both correct, intended behaviour (§1.4.4's per-document dirty gate has no
"but it's not the active document" exception) — this was classification (b)
from this file's own suggested-fix section: a fuzz-DRIVER precondition bug,
not a production defect. Fixed per option (a): `driver::State` now carries
`seed_doc`, the `DocumentId` `App::new` mints for the seeded document,
captured once before any action runs; `checks::drive_end_of_session_checks`
skips the whole end-of-session undo/redo drive (and, with it, `UNDO-TOTAL`/
`REDO-TOTAL`) whenever `seed_doc` is no longer in `app.documents` — a
discarded document has no undo history left to prove anything about, the
same "inert by design" shape as the G5/G15 skip conditions already there.
Regression test `seed_discarded_by_dirty_close_guard_skips_undo_total`
(`crates/rune-fuzz/tests/tripwire.rs`) pins the exact repro below (not
`#[ignore]`d, runs under `cargo test --workspace`). The pinned seed in
`proptest-regressions/human_session.txt` (`cc b82c8617...`) replays clean.

## Minimal repro

```rust
use rune_fuzz::action::Action;
use rune_fuzz::driver;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

let actions = vec![
    Action::Type("hello world".to_string()),
    Action::Key(KeyInput { code: KeyCode::F1, mods: Mods::NONE }),
    Action::Key(KeyInput { code: KeyCode::Char('¡'), mods: Mods::NONE }),
    Action::Key(KeyInput { code: KeyCode::Char(' '), mods: Mods::NONE }),
    Action::Key(KeyInput {
        code: KeyCode::Char('c'),
        mods: Mods { ctrl: true, ..Mods::NONE },
    }),
    Action::StaleConfirmTimeout(4294967295),
    Action::Type("\"quoted\" 'text'".to_string()),
];
let result = driver::run("/fuzz/doc.md", "", &actions);
// result.violation == Some(Violation {
//     id: "UNDO-TOTAL",
//     message: "content after undoing to journal_pos == 0 does not match \
//                the seed: seed=\"\" after=\"# Help\\n\\n## Global\\n...\"",
// })
```

Also pinned in `crates/rune-fuzz/proptest-regressions/human_session.txt`
(seed `cc b82c8617...`) — proptest replays it on every future `make
test-fuzz`.

## Root cause (confirmed, via temporary driver instrumentation — reverted,
not part of any commit)

1. `Type("hello world")` dirties the seeded document (`DocumentId(1)`),
   never saved.
2. `F1` mints and switches to the virtual Help document (`DocumentId(2)`).
3. `^C` (`GlobalCommand::QuitChord`) reaches `pane::handle_quit_key`, which
   scans ALL open documents (not just the active one) via
   `first_unpreserved_dirty_doc` — finds `DocumentId(1)` dirty with no `db`
   binding (this fuzz driver never wires one up) — and raises
   `Modal::Guard(GuardPrompt { doc: DocumentId(1), kind: DirtyClose })`
   INSTEAD of arming `pending_quit`. This is why `pending_quit` stays `None`
   for the rest of the session (confirmed by instrumentation) — the ctrl+c
   never even reached the quit-chord state machine.
4. The stale `ConfirmTimeout` is inert, as designed (`confirm_gen`'s own
   docs) — a no-op.
5. `Type("\"quoted\" 'text'")` expands to one key per char. The `'d'` in
   `quoted` matches `banner::DIRTY_CLOSE_DISCARD`'s key
   (`banner::handle_dirty_close_key`), which calls
   `workspace::close_now(app, DocumentId(1))` — discarding and permanently
   closing the seeded document, even though it was NOT the active document
   at the time (Help was). `close_now` only reassigns `app.active` when the
   CLOSED document was active (it wasn't here), so `app.active` stays on
   Help and `app.documents` shrinks to `{Help}` only.
6. At session end, `driver/checks.rs::restore_editor_focus` presses `F1`
   again (since `help_doc == Some(active)`), calling
   `workspace::toggle_help`. Its own switch-back target
   (`help_return_to.filter(|t| documents.contains_key(t))`) now fails
   (`DocumentId(1)` no longer exists), falls through
   `.or_else(documents.keys().find(other != id))` (no other document
   exists either), and `.unwrap_or(id)` lands back on Help itself — a no-op.
7. The undo/redo drive therefore runs against the Help document (whose
   `journal_pos`/`journal_len` are trivially `0`/`0`, never edited), and
   `UNDO-TOTAL` compares Help's synthetic markdown against the ORIGINAL
   empty seed — a mismatch that is real (the seeded document's content is
   gone forever, discarded by design), but not what `UNDO-TOTAL` exists to
   detect.

This is production working exactly as designed at every step (the Guard,
the discard key, and `close_now`'s active-reassignment rule are all correct
and desirable behaviour) — the defect is in the FUZZ DRIVER's own
assumption, stated as fact in two places that both need revisiting:
- `driver/checks.rs::restore_editor_focus`'s docs ("this driver never opens
  more than one non-Help document" / "`F1` again... switches back to
  whatever was active right before Help was last activated, which is always
  the seeded document") — false once a Guard-armed quit chord discards it.
- `invariant/undo.rs::undo_total`/`redo_total`'s implicit precondition that
  the seeded document is still open and reachable by session end.

## Suggested fix shape (not implemented — needs someone who owns the
driver's end-of-session drive)

The end-of-session drive needs to either (a) detect that the seeded
document was discarded and skip `UNDO-TOTAL`/`REDO-TOTAL` entirely for that
session (a discarded document has no undo history left to prove anything
about — this is the G5/G15 kind of "inert by design" case, not a coverage
hole), or (b) never let the fuzz driver's own dirty-close Guard reach a
DISCARD outcome in the first place (e.g. by not driving the generic
per-char `Type` payload while any modal is up, seeding stronger
documentation of the discard risk into the generator instead). Whoever
picks this up should confirm which of the two the maintainers intend before
choosing — this file only pins the repro and root cause, not the fix.
