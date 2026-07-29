# TODO — SAVE-INFLIGHT-SM fires on F1 while a save is in flight

**Found by:** the `make test-fuzz RC=50000` soak run while verifying the
`fix-sync-idempotent` branch (which fixes an unrelated defect — see the now-
deleted `TODO-sync-idempotent-link-reveal-lag.md`, folded into that branch's
commit history). Confirmed to reproduce identically on `0ae098c` (this
branch's own base, before any of that branch's changes) — genuinely
pre-existing, not introduced by the sync/reveal fix.

**Status:** open, out of scope for `fix-sync-idempotent` (that branch's scope
is the display pipeline's reveal/scroll idempotence; this is the save state
machine).

## Minimal repro

```rust
use rune_fuzz::action::Action;
use rune_fuzz::driver;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

let actions = vec![
    Action::Type("hello world".to_string()),
    Action::Key(KeyInput { code: KeyCode::Char('s'), mods: Mods { sup: true, ..Mods::NONE } }),
    Action::Key(KeyInput { code: KeyCode::Char('A'), mods: Mods::NONE }),
    Action::Key(KeyInput { code: KeyCode::Char(' '), mods: Mods::NONE }),
    Action::Key(KeyInput { code: KeyCode::F1, mods: Mods::NONE }),
];
let result = driver::run("/fuzz/doc.md", "", &actions);
// result.violation == Some(Violation {
//     id: "SAVE-INFLIGHT-SM",
//     message: "save_in_flight went true->false on Key { input: KeyInput { code: F1, .. },
//                command: None }, not a SaveDone",
// })
```

Equivalently: seed an empty doc, type `hello world`, press `⌘S` (arms a save
`Cmd`, `save_in_flight` -> `true`), keep typing (`A`, ` `) with the save
still outstanding, then press `F1` (`GlobalCommand::Help`, switches
`app.active` to the virtual Help document). `save_in_flight` flips back to
`false` on that `F1` step even though no `Msg::SaveDone` was ever delivered
— `SAVE-INFLIGHT-SM`'s own invariant (`crates/rune-fuzz/src/invariant/
session.rs::save_inflight_sm`) says that transition may only happen on a
genuine save completion.

## Already recorded in the fuzz regression corpus

`crates/rune-fuzz/proptest-regressions/human_session.txt` already carries
this case's seed (`cc 94f569b2...`) — proptest will re-run it first on every
future `make test-fuzz`, so it will keep surfacing until fixed.

## Working hypothesis (unconfirmed)

`GlobalCommand::Help` / `workspace::toggle_help` switches `app.active` to the
virtual Help `DocumentId`. `save_in_flight`/`save_pending_version` live on
`Document` (per-document fields, see `document.rs`'s `Document` struct
docs), not on `App`. If toggling to Help reads/reports `is_dirty`/
`save_in_flight` off the NEWLY active (Help) document — which never has a
save in flight — rather than the document the save was actually issued
against, the snapshot the fuzzer observes would show `save_in_flight: false`
for the wrong reason (comparing the wrong document's field, not a real
state-machine transition on the right one). Needs someone who owns
`rune-tui`'s `workspace::toggle_help`/`Snapshot::capture` to confirm whether
`Snapshot`'s `save_in_flight` is doc-scoped correctly across an active-document
switch, or trace the actual `update()` path `F1` takes with the save still
outstanding.
