# TODO

## Linux package repositories

Native Linux packages (`.deb`, `.rpm`, `.pkg.tar.zst`) now build and publish via nfpm
on each release. Currently packages are point downloads from GitHub Releases only —
no automatic updates. Follow-up work for usable Linux distribution support:

- **AUR** — `rune-bin` package for Arch users
- **Fedora COPR** — enable `dnf upgrade rune`
- **Hosted apt repo** — e.g. Cloudsmith, for `apt upgrade rune`
- **cargo-dist shell installer** — `curl ... | sh` one-liner

Prerelease versions skip package publishing (gated in workflow); future repos should
inherit that behavior.

## CLIP-OSC52 fails on cut inside reading view (found by `make test-fuzz`)

`make test-fuzz` (the `human_session` fuzzer) deterministically reproduces a `CLIP-OSC52`
violation, unrelated to the `SCROLL-IN-DOC` invariant work that surfaced it (that work only
added a new, independent check; this failure trips an existing, older invariant). Minimal
repro script (see `crates/rune-fuzz/artifacts/proptest-regressions/human_session.txt`):

```
type hello world
diverge-disk
deliver-db-all
key char:m ctrl        (merge chord)
deliver-db-all
key char:y shift+sup    (redo? — see MergeState)
key char:m ctrl
deliver
key right shift          (extend selection)
key char:P ctrl           (toggle reading view — ReadOnly::Reading)
key char:x sup             (cmd+x, cut)
```

Panic: `CLIP-OSC52: no OSC 52 raw chunk decoded to the selected text "f"; raw chunks emitted: 0`

Cutting (cmd+x) a selection while the document is in reading view (read-only) emits no OSC 52
clipboard chunk at all — not even a copy-only fallback. Needs a real fix: either cut in a
read-only document should still copy the selection to the clipboard (just skip the delete), or
the fuzzer's own `CLIP-OSC52` invariant needs a documented read-only carve-out if that refusal
is intentional product behavior. Not investigated further — out of scope for the viewport-clamp
fix this TODO was filed alongside.

## Named drafts left behind on discard

A launch positional that does not exist on disk yet is opened as a named scratch row
(`bootstrap_new_file` in crates/rune-cli/src/db_bootstrap.rs, `create_named_scratch`). Closing
it with ^W and answering discard leaves that row in the recovery store, so it is offered back
on the next launch of the same path. Untitled drafts now go through `Store::forget_scratch` on
close; named drafts should too once it is decided whether a discarded named draft must still be
offered back.

## Double adoption of a dead session's draft

Adopting a recovered scratch row at launch claims nothing in `session_documents` until the first
journal write (see `adopt_scratch_doc` in crates/rune-tui/src/db_ack.rs and `reconstruct_scratch`
in crates/rune-db/src/scratch.rs). Two launches racing before either journals can both adopt the
same row. Now that `forget_scratch` can delete a row and SQLite reuses the freed rowid
(`documents.id` has no AUTOINCREMENT), the loser can journal onto a rowid that was re-minted for
a different draft. Fix candidate: claim the row in `session_documents` at adoption time, before
binding.

## Chord labels hard-coded in user-facing text

Several messages and footer rows spell a chord by hand instead of reading it from the binding
table through `global::label_for` / `hint_for`: `^M` in `materialize_ack/reactions.rs` ("^M to
merge") and `db_ack.rs` ("[^M]erge"), and `^K` in `footer_modes.rs`. The rename hint in
`reactions.rs` rotted exactly this way when rename moved from `^R` to `F2` and was fixed in that
change, and the find panel's "finish the merge first" refusal now reads its chord from the table;
the rest will rot the same way the day their chord moves. Fix: one chokepoint that formats a
message's chord from its `GlobalCommand` (or pane command), and tests that assert the label comes
from the table.

## `replace_history` and `command_history` tables were added in place

`CONSTITUTION.md` names a nullable column as the one schema change that may land under the same
`rune-vN.db` filename; a new table is supposed to ship as a new file. `command_history` set the
precedent of appending a `CREATE TABLE IF NOT EXISTS` to `SCHEMA` without a `SCHEMA_VERSION`
bump, and `replace_history` (the Replace field's own history, same shape as `search_history`)
followed it, because a bump would strand every unsaved document's recovery journal in the old
file for the sake of a history table nobody depends on. Decide whether the rule or the precedent
wins: either the constitution admits "a new table with no foreign keys" as an additive change,
or both tables move to `rune-v3.db` together with a migration that carries the journals across.

## `^⇧P` is swallowed silently while the find panel has focus

`commands::reading::toggle` returns without a word unless the editor or the palette owns focus,
and `GlobalCommand::ToggleReadOnly` is `BarPolicy::LeaveOpen`, so pressing the reading-view chord
with the find panel focused consumes the key and changes nothing. Either the toggle should apply
to the active document regardless of which overlay holds the keyboard (the panel already works on
a read-only document), or it should say why it refused.

## The fuzzer never sees project search results

`driver/step_exec.rs` drops every `CmdKind::ProjectIndex` and `CmdKind::ProjectQuery` the update
loop spawns, and the session fixture seeds one file with no project root. So `cluster_find`'s
Project-scope steps (open the top hit, `⇧⏎` replace-all across files) always run against an empty
result list, and `FIND-REPLACE-NO-DISK` / `FIND-WALK-DRAINS` hold trivially. Fix candidates: seed a
small multi-file `Mem` project under a root in `Session::open` and run the index/query cmds inline
the way `ReadFile` replies are already driven, so a real walk (tab limit, queued loads, evicted
origin) is reachable from generated sessions.

## Multi-caret Enter in an ordered list rescans the run once per caret

`commands::edit_renumber::plan` calls `rune_md::continuation::renumber` for every caret that inserts
an ordered item, and each call walks the sibling run to its end. N carets in one run of M items
cost O(N x M) `parse_prefix` calls. Fix candidate: scan the run once, then give each caret the
suffix after its own line, cutting at the first `Claims` overlap.

## `syntax_to_wrap` mixes column units on rendered table lines

On a rendered (not revealed) table line the syntax-to-buffer map is the identity, so a syntax column
is a raw source byte column. `wrap_table_line` in `crates/rune-syntax/src/wrap/table.rs` gives the
extra visual rows of a Wrapped or Pivoted table line `start_col` values that sum rendered text bytes
(box-drawing characters, padding). `syntax_to_wrap` in `crates/rune-syntax/src/wrap/query.rs` then
compares the source column against those bounds, so a source column lands in an arbitrary visual row
of the line, and `wrap_to_syntax` can return a column past the source line end. This is how Down
became a no-op at the end of a wide table's last row. Vertical motion now avoids measuring a caret on
a rendered table row, but any other caller of `syntax_to_wrap` on such a line inherits the mismatch.

Candidate fix: segment 0 owns the whole source column range of the line (row 1 already claims all
source bytes through `claim_whole`). Extra rows claim a zero-width source extent. Then `syntax_to_wrap`
maps every source column to segment 0, and `wrap_to_syntax` from an extra row maps to a defined source
column.

## CUR-BOUNDS: a cursor outlives a paste that replaces the document (found by `make test-fuzz`)

`human_session` fails `CUR-BOUNDS` with `cursor id=1 position=68 anchor=0 content.len()=11
content="hello world"`: after the paste the buffer holds 11 bytes, but the primary cursor still has its
position at byte 68. Proptest regression `cc
29aeab10d520cc3e42c5f597e0e6e449b4dab888e52af4afc2479f42d29057ef`. Reproduces deterministically,
also on a checkout with the wide-table Down fix (`RUNE_FUZZ_REPLAY=<script> cargo test -p rune-fuzz
--test replay`). The script, verbatim (the seed is a mixed CJK and emoji line, a shift+Down
selection, ctrl+P, three newlines typed, then cmd+V with the clipboard set to "hello world"):

```
content \u{4f60}\u{597d}\u{4e16}\u{754c} \u{1f642} mixed CJK and emoji content \u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30b9}\u{30c8}\n
key down s---
key char:p --c-
type \n\n\n
type 
key char:v ---u
clip hello world
type 
```

Suspected cause: a selection whose head sits at the end of the old content survives the paste that
replaces the whole document in a new draft, so the cursor is never clamped to the new content. Not
root-caused.

## TABLE-ROW-WIDTH: a table inside a blockquote renders rows of unequal width (found by `make test-fuzz`)

`human_session` fails `TABLE-ROW-WIDTH` with `table_group 0: row 2 has summed width 15, but row 0
(same group) has width 13`. The document ends as `> |---|---|\n> |---|---|\n\n`, i.e. delimiter-only
tables inside a blockquote, typed from an empty seed. Proptest regression `cc
a34dc24fe8ca3633186aba9b660b7bc8728877acdc17b766e03dadeaa6cfd35a`. Reproduces deterministically,
also on a checkout with the wide-table Down fix (`RUNE_FUZZ_REPLAY=<script> cargo test -p rune-fuzz
--test replay`). The script, verbatim:

```
content 
type > 
type |---|---|
key up -a-u
type \n\n\n
key char:c ---u
key pageup ----
key down sa--
key right ----
key char:  ----
```

Not root-caused.

## SAVE-CLEAN-MATCHES-DISK fired once (found by `make test-fuzz`)

One 1500-case `make test-fuzz` run reported `SAVE-CLEAN-MATCHES-DISK` with `disk=None
content="h {feff}hello"`: the buffer held a U+FEFF byte-order mark mid-text and the invariant found
no disk content to compare it against. The run's artifact was not kept and the failure has not
reproduced since (the default 512-case run passes), so there is no script. Next step: rerun with a
large `RC=` and a few `RS=` seeds until it recurs, then keep the artifact and root-cause it.

## rune-fuzz invariant roster is incomplete

The roster in the `crates/rune-fuzz/src/lib.rs` doc comment ("41 total") omits invariants that
`src/invariant/` does check: `CUR-CELL-SYNC`, `FIND-PREVIEW-PURE`, `FIND-REPLACE-NO-DISK`,
`FIND-WALK-DRAINS`, `MERGE-UNDO-NEVER-COMPLETES`, `OVERLAY-TITLE-EXCLUSIVE`, `PALETTE-FOCUS-STABLE`,
`PALETTE-GUARD`, `SCROLL-IN-DOC`. The hand-kept list and count drift each time an invariant lands.
Candidate fix: add the missing entries, and make the roster checkable — e.g. a test that every id
string `check_all` can emit appears in the doc roster, so drift fails `make test`.

## Vertical motion can land between CR and LF on a CRLF line

On `abcdef\r\nA\r\nC`, pressing End on line 0 and then Down puts the caret at byte 10, between the
`\r` and `\n` of line 1, instead of at byte 9 (before the `\r`). `WrapSnapshot::byte_col_from_visual`
walks the row's span text up to the end of the row without stopping at the line's content end, and
the caller's `Buffer::line_col_to_offset` clamps to the line end (the `\n`), not the content end.
`delete_line` works around it with its own clamp in `landing_offset`; the motion paths in
`nav_scroll.rs` (`move_row`), `multi.rs` (`visual_col_on_line`) and `document/sync.rs`
(`snap_cursor_to_row`) do not. Candidate fix: one clamp-to-content-end chokepoint those four callers share.

## Word wrap off: a very long line costs O(line) per frame

With wrap off, `build_rows` in `render/mod.rs` builds a `Cell` for every grapheme of every visible logical line
and `hscroll::scroll_columns` then drops the leading ones. A single 1 MB line (200k words) costs about 86 ms per
frame in a release build, with the caret at the end of it. Every pass between the two (`code_bg::fill_row`,
`overlay::place_caret`, `paint_range`, the decor prefix) indexes a row by cumulative cell width from column 0, so
the leading drop cannot move before cell building without making all of them window-relative. A trailing clip
alone would not help the worst case: the caret at the end of the line keeps `scroll_col` near the line length.
The edit path is O(line) as well: one key press on that line takes about 38 ms (`caret_cell` and `visual_col` walk
the whole line in `document/sync.rs`, plus the per-edit syntax and wrap passes). Split candidate: pass a
`Range<usize>` window of visual columns into `segment_cells` and carry a per-row column origin that the overlay
passes subtract, and give `visual_col` a line-start index. Not guarded by `make perf-guard` until it is fixed.

## Word wrap off: no horizontal wheel scroll

With wrap off the view scrolls sideways only to follow the primary caret. A horizontal wheel or
shift+wheel notch does nothing to `scroll_col`, so a user with the mouse cannot look at the far end of a
long line without moving the caret there. Candidate fix: a scroll command that sets `scroll_col` directly
and arms a column analogue of `ScrollMode::Independent`, so `reconcile_col` snaps the caret instead of the view.

## Word wrap off: dragging a selection does not scroll past the right edge

A mouse drag that selects text on an unwrapped line cannot reach text beyond the pane's right edge: the view
does not auto-scroll sideways while the pointer is held outside the pane. Needs a drag tick (a timer
message, never a sleep) that advances `scroll_col` while the pointer is held outside the pane.

## Word wrap is not remembered across restarts

`Viewport::wrap_mode` lives on the in-memory `Document` and is never written to the recovery store, so every
launch and every reopened tab starts wrapped. The choice is per document and per session only. Persisting it
needs a column in a new `rune-vN.db` file or a nullable column on an existing table, per the schema rules in
the constitution.

## Word wrap off: horizontal scroll follows only the primary caret

`Document::scroll_to_cursor` feeds only the primary cursor's column to `Viewport::reconcile_col`. A secondary
cursor beyond the right edge of an unwrapped line is painted nowhere and nothing scrolls to reveal it, so a
multi-cursor edit can change text the user cannot see. Candidate fix: reveal the cursor the last command
moved or added, not only the primary one.
