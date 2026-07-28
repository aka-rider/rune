# `repros/` — checked-in fuzz replays

Every `*.rune` file directly under this directory (not `strict-known/`, not
this `README.md`) is replayed forever by `tests/replay.rs`, which runs on
every `make test`/`cargo test --workspace`. A script that lands here and
passes stays green permanently; a script that starts failing is a real
regression.

## Workflow: moving a fuzz catch here

1. `make test-fuzz` (or `make -C rust test-fuzz`) catches an invariant and
   writes `crates/rune-fuzz/artifacts/<id>-<hash>/{report.txt,script.rune}`
   (gitignored — `rust/.gitignore`).
2. Root-cause and fix the underlying bug. **Do not** move the script here
   while it is still red — `/Users/xiii/Developer/rune/TODO.md:60`'s
   convention ("re-add each corpus entry in the SAME commit as its fix")
   applies here too: a repro belongs in this directory only in the same
   commit as the fix that makes it pass.
3. **Move** (do not copy) `artifacts/<id>-<hash>/script.rune` into this
   directory as `<id-lowercased>-NN.rune`, where `NN` is the next unused
   two-digit index for that id (`redo-clear-01.rune`,
   `redo-clear-02.rune`, ...).
4. Run `cargo test -p rune-fuzz --test replay` to confirm the new file
   replays clean, then commit the script alongside the fix.

## `strict-known/`

If a script only fails because of `rune-md`'s `strict-invariants` feature
(known-open comrak sourcepos panics on lone-`\r`-next-to-tab inside nested
containers — see `crates/rune-md/TODO.md`, plan Gotcha G1) and not because
of a real Phase-1 bug, put it in `repros/strict-known/` instead.
`tests/replay.rs` deliberately skips that subdirectory, so it never turns
`make test` red for a defect this crate isn't chartered to fix. Also add a
line to `rune-md/TODO.md` recording it. Do not delete a `strict-known/`
entry — it is evidence, not noise.

## Seed script

`tripwire-clean.rune` is the WP4 tripwire's hand-written "normal human
session" (`tests/tripwire.rs`'s `tripwire_script()`/`FIXTURE`), encoded via
`script::encode` and checked in here so `tests/replay.rs` is never
vacuously replaying zero scripts.
