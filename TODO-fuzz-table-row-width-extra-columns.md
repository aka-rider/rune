# TODO — `TABLE-ROW-WIDTH` fires after typing extra `|`-delimited columns into one row of a boxed table

**Status: RESOLVED.** Root cause was NOT the Grid column-width computation
this entry originally suspected (`table::layout::col_widths`/`grid_row` —
both already measure and render every row using the SAME `n_cols`-sized
width vector, so a ragged row's extra cells, silently dropped by comrak's
own table parser past the header's column count, never reach either of
those functions in the first place). The real defect lived one layer up,
in cursor rendering: `rune-tui`'s `render::overlay::apply_cursor_overlays`
maps a cursor's buffer byte offset to a rendered column via
`SyntaxSnapshot::buffer_to_syntax`, a generic scheme that only ever
accounts for CONCEALED inline markup (`emit`'s `hidden` bookkeeping) —
it has no idea a table row's raw source line was wholly SUBSTITUTED for a
rendered box. A ragged row's dropped cells stay part of the raw line, so
typing into them grows the raw byte column without bound while the box's
own rendered width stays fixed; once the cursor's mapped column reached or
passed the row's actual cell count, `place_caret`'s "caret sits past the
last visible char" fallback (correct for an ordinary unboxed line) kicked
in and appended a synthetic one-cell EOL cursor — making that ONE row a
cell wider than every other row in its table group.

Fixed by making `place_caret` boxed-row-aware (`crates/rune-tui/src/
render/overlay.rs`): a Grid/Wrapped table's own content and border rows
never take the append branch — the caret clamps onto the row's own last
cell instead, so a boxed row's width can never grow past what every
sibling row in its group already has, by construction rather than by
coincidence of the column math staying in range. `apply_cursor_overlays`
looks up `boxed` from the cursor's own wrap segment (`WrapSegment::table`)
before calling `place_caret`.

Regression: `crates/rune-tui/tests/tui_render_tables.rs`'s
`caret_inside_a_ragged_rows_dropped_cells_never_widens_that_rows_box` —
measures every table-group row's summed `Cell::width` (the same quantity
the fuzzer's `TABLE-ROW-WIDTH` invariant checks) via `render::build_rows`
directly rather than the backend terminal grid, since the appended
synthetic cell reads as ordinary editor-background padding once blitted
to a fixed-width terminal row and would otherwise hide the regression.
Confirmed to fail (`[15, 15, 15, 15, 15, 16, 15]`) against the pre-fix
`place_caret` and pass (uniform `15`) against the fix.

Seed `cc 5f23e392...` (`crates/rune-fuzz/proptest-regressions/
human_session.txt`) now replays clean — left pinned, as always, so any
future regression on this exact shape is caught immediately.

The second seed noted below, `cc a9393f5d...`, was replayed directly
(empty doc, `Tab`, `F1` opening the generated Help document, a space, an
inverted `¡` char, `Ctrl+C`, a stale-confirm timeout, then typing "hello
world") and produces no violation on this tree — left pinned, unexamined
beyond that replay, per its own original note below.

---

Original entry, preserved for the record:

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

A second, unrelated new seed (`cc a9393f5d...`) was also saved by the same
soak run but did not reproduce a failure by itself in this run — proptest
saves every case it ever explores that could shrink to a future failure,
not just the ones that failed this run; left in place, unexamined.

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
