# TODO — `TABLE-ROW-WIDTH` fires after typing extra `|`-delimited columns into one row of a boxed table

**Found by:** `make test-fuzz RC=5000`, a short soak run to confirm the
`fix-table-sync` branch's own `SYNC-IDEMPOTENT` fix (`TODO-fuzz-sync-
idempotent-table-scroll.md`, now closed by that fix) holds. Recorded here
per that task's own contingency clause: this is a DIFFERENT invariant
(`TABLE-ROW-WIDTH`'s per-row summed-width agreement, not `SYNC-IDEMPOTENT`'s
render/scroll idempotence) and a different action shape (typing new `|`
columns into an existing row, not a scroll command) — confirmed distinct
from `fix-table-sync`'s own finding, then NOT chased, per that task's scope.
Also distinct from the two already-`RESOLVED` `TABLE-ROW-WIDTH` entries in
`TODO.md` (both traced to the Grid column-width/grapheme-run measurement
mismatch) — this is plausibly a THIRD such defect, but that has not been
verified.

**Status:** open. Pinned in `crates/rune-fuzz/proptest-regressions/
human_session.txt` (seed `cc 5f23e392...`), so `make test-fuzz` will keep
replaying and failing on it until fixed. A second, unrelated new seed
(`cc a9393f5d...`) was also saved by the same soak run but did not
reproduce a failure by itself in this run — proptest saves every case it
ever explores that could shrink to a future failure, not just the ones
that failed this run; left in place, unexamined.

## Minimal repro (frozen artifact)

`crates/rune-fuzz/artifacts/table-row-width-b576c1b0/report.txt`
(gitignored — regenerate with `make test-fuzz RC=5000` if it no longer
exists locally):

```rust
use rune_fuzz::action::Action;
use rune_fuzz::driver;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};

const DOC: &str = "# Doc\n\n| Name | Age |\n| :--- | ---: |\n| Alice | 30 |\n| Bob | 25 |\n\ntail\n";

let actions = vec![
    Action::Key(KeyInput { code: KeyCode::PageDown, mods: Mods::NONE }),
    Action::Key(KeyInput { code: KeyCode::Up, mods: Mods::NONE }),
    Action::Key(KeyInput { code: KeyCode::Left, mods: Mods { alt: true, ..Mods::NONE } }),
    Action::Type("| a | b |".to_string()),
    Action::Key(KeyInput { code: KeyCode::Char('c'), mods: Mods { sup: true, ..Mods::NONE } }),
    Action::Key(KeyInput {
        code: KeyCode::Char('c'),
        mods: Mods { ctrl: true, ..Mods::NONE },
    }),
    Action::StaleConfirmTimeout(4294967295),
];
let result = driver::run("/fuzz/doc.md", DOC, &actions);
// result.violation == Some(Violation {
//     id: "TABLE-ROW-WIDTH",
//     message: "table_group 0: row 7 has summed width 16, but row 2 \
//                (same group) has width 15",
// })
```

The frozen snapshot's content after the drive is `"# Doc\n\n| Name | Age
|\n| :--- | ---: |\n| Alice | 30 |\n| Bob | 25 | a | b ||\n\ntail\n"` —
typing `"| a | b |"` mid-row appended two more `|`-delimited cells to the
`Bob` row, so that row now has more (and differently-sized) columns than
every other row in the same table group. Whoever picks this up should
start from the same Grid column-width computation the two `RESOLVED`
`TABLE-ROW-WIDTH` entries in `TODO.md` point at (`rune-md`'s `table::
layout::col_widths`/`grid_row`), but for the specific case of a row whose
OWN cell count differs from the table's header-derived `n_cols` — comrak's
own table parser may be tolerant of a ragged row (silently dropping or
padding extra cells) in a way this port's column-width sizing isn't
accounting for consistently across rows.
