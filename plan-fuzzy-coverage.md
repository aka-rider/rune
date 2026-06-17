# Widen & Deepen the Rune Fuzzing Suite — Invariant Catalogue

## Context

`plan-fuzzy-testing.md` (Phase 0/1) is implemented: a deterministic in-process driver steps
`workspace.Model` through decoded events, drains every `tea.Cmd` inline, and calls
`CheckInvariants(Snapshot)` after each settled message. Today it checks **R1–R7** (cell/cursor
render), **T1–T2** (tab bar), **M1–M2** (model), plus stubs for **SHADOW** and **DL1**.

The goal now is to raise the **bug-catching density per fuzz input** — catch more real defects on
every settled message — and to reach deep precondition states faster. Direct inspection of the
implemented harness surfaced five facts that shape the work:

1. **The SHADOW oracle is inert.** `invariant.go:191` only fires when `s.MirrorContent != ""`, but
   the driver (`driver.go:56–69`) never populates `MirrorContent` — it keeps no mirror and never
   drains edits. The content-correctness oracle, the centerpiece of the design, currently never runs.
2. **DL1 is dead code.** `driver.go` calls `CheckInvariants` only; `CheckDataLossInvariants`
   (`invariant.go:279`) is never invoked.
3. **The Snapshot is thin** (`invariant.go:18–34`): `Content`, `Cells`, `CursorOffsets` (positions
   only — no anchors/IDs/selections), `Tabs{Path}`, `ActiveTabIdx`. It omits the full cursor set,
   buffer `lineStarts`/`version`, the `DisplaySnapshot`, selection ranges, focus pane, footer/guard
   state, dirty flags, and terminal dimensions. Most new families need a wider Snapshot — but several
   gated accessors already exist and go unused: `FuzzSnapshot()`, `FuzzCursors()` (`textedit_fuzz.go`),
   `CursorOffsets()`, `Selections()` (`textedit.go:386,395`), `footer.InGuard()`/`GuardKind()`.
4. **The text channel inserts only the first rune.** `eventToMsg` (`keytable.go:74–80`) decodes a
   `KindText` event to `tea.KeyPressMsg{Code: runes[0]}` — the remaining runes are length-counted and
   discarded. Every multi-char `KindText` seed in `session_fuzz_test.go` (e.g. the "hello" seed) inserts
   a *single* character; its comment is wrong. Bulk text enters only via `KindPaste` (`tea.PasteMsg`,
   full `ev.Text`, `keytable.go:83–87`). Multi-line / long-line / markdown states are therefore O(events)
   to reach by mutation — the dominant reason deep editor states are rare.
5. **The key alphabet has no markdown metacharacters.** `bindingTable` (`keytable.go:55–60`) covers
   `a–z`, space, `.`, `,`, `\n` only — no `* # | [ ] ! _ - > ``. Via `KindKey` the fuzzer can never type
   a markdown token, so the Rendered/Revealed-span machinery (`syntax_map.go`, `table_rows.go`,
   `image_rows.go`) is reachable only through `KindText`/`KindPaste`, and no seed carries markdown.
   D1–D3 and the new display family below currently exercise plain text only — the Rendered path is dead
   until generation is fixed.

So the work is **deepen** (make the half-wired oracles actually fire), **unblock generation** (so the
display oracles can reach Rendered spans at all), **then widen** (new families).
All new code stays under `//go:build fuzzing`; production compiles none of it (D10).

---

## Design principles (carried from the existing plan)

- **Cheap, every settled message.** Each invariant is a pure function of the Snapshot (or a
  `(prev,msg,next)` triple). No I/O except DL-family (gated to save events / Phase-2 only).
- **Levels (addendum D16).** **L0** = stateless `Check(next)`; **L1** = `CheckTransition(prev,msg,next)`;
  **L2+** = stateful `Monitor` automata (`Reset()` per shrink replay).
- **Severity.** DATA-LOSS = HARD STOP. Render/structure = HIGH. Heuristic/“may need tuning” = MEDIUM.
- **First violation freezes** (driver behavior unchanged); the new checks slot into the same loop.

---

## Family 0 — Deepen: wire the inert oracles (highest value/effort)

| ID | Level | Checks | Mechanism (grounded) | Sev |
|----|-------|--------|----------------------|-----|
| **SHADOW** (fix) | L0 | buffer `Content()` equals an independent replay of recorded edits | Driver-owned `store` already journals every edit to surface `"main"` (`workspace.go:773 journalEdit`). Add `(*Store) AllEdits(surface) ([][]buffer.AppliedEdit, error)` — **grouped by batch** (one `AppendEdit` row = one batch), reusing the journal's JSON codec. After each settled msg, driver replays into a `mirror string`: fold **each batch in ascending-`Start` order** via `mirror = mirror[:e.Start] + e.Insert + mirror[e.Start+len(e.Deleted):]`, baseline `""` (fresh Untitled sessions). `AppliedEdit.Start` is the **post-shift NEW-buffer offset** (`buffer.go:150–163`), so ascending-`Start` processing makes each edit's baked-in shift align with the running mirror displacement — **correct for multi-cursor batches** (single-cursor batches have shift 0, so the order is moot). The naive flat fold (array/descending order) mis-reconstructs multi-cursor edits. Then set `snap.MirrorContent`. Also validates **journal fidelity / coalescing** (a data-loss property). | HARD STOP |
| **DL1** (thread) | L0 | file-on-disk == `buffer.Content()` after a save settles | In `drainMsg`, when `msg` is `workspace.FileSavedMsg`, call the existing `CheckDataLossInvariants(snap)` (Phase-2 `OpenAt` + real workdir; in-memory sessions skip via the existing file-read-error guard at `invariant.go:284`). | HARD STOP |
| **DL2** | L0 | `store.LatestSnapshot(docID)` == `buffer.Content()` after flush settles | `flushDelay=0` under fuzzing, so `CreateSnapshot` (`workspace.go:474`) runs as a drained Cmd. Reuse `CreateSnapshot`/`LatestSnapshot`. Reads `snap.DocID` — carried on `Snapshot`, populated `DocID: m.docID` in `FuzzInspect`; no separate accessor (`m.docID` is private at `workspace.go:120`). | HARD STOP |
| **DL3** | L1/L2 | undo→redo restores exact `Content` + cursor offsets | Drive `Undo`/`Redo` bindings; capture content+cursors before, assert equal after the round-trip. Exercises `UndoTarget`/`RedoTarget` (`journal.go:116,145`) + 300 ms coalescing under the injected clock. | HARD STOP |
| **DL4** (undo-stack RT) | L1/L2 | full undo to bottom restores the *original* `Content`; full redo to top restores the *latest* | Extends DL3 from one step to the whole stack — accumulation/coalescing bugs hide *between* steps. Drive `Undo`×N then `Redo`×N; capture content at both ends. | HARD STOP |
| **DL5** (undo-truncate) | L1 | after `edit → undo → edit'`, a subsequent redo is a no-op (future truncated) | `AppendEdit` truncates rows with `seq > currentSeq` (`journal.go:28–34`, the `DELETE FROM events WHERE seq > ?` inside `(*Store).AppendEdit`). Seed the cluster; assert post-`edit'` redo leaves `Content` unchanged. | HARD STOP |
| **DL6** (coalesce-bound) | L1 | a single-char insert >300 ms after the prior — or right after a whitespace insert — starts a NEW journal entry | Coalescing boundaries (`journal.go:39–75`) under the injected clock. SHADOW validates journal *fidelity*; this validates undo *granularity*. | MEDIUM |

> Wiring SHADOW alone converts the engine's primary oracle from a no-op into a live check — the single
> biggest efficiency gain available.

---

## Family 1 — Layout & render-overflow (screenshot-provable, resize-stressed)

The current R-family checks *cell content* but never *geometry*. Resize events already exist in the
grammar; nothing asserts the frame fits the terminal (CLAUDE.md §4.4 `MaxWidth`/`MaxHeight` contract).

| ID | Level | Checks | Mechanism | Sev |
|----|-------|--------|-----------|-----|
| **L1** | L0 | every composed frame line's display width ≤ terminal width | Snapshot gains `Frame string` (`m.View().Content`) + `Width int`. Split on `\n`; per line `lipgloss.Width(line) ≤ Width`. Canonical “no overflow past the terminal.” | HIGH |
| **L2** | L0 | frame line count ≤ terminal height | `strings.Count(Frame,"\n")+1 ≤ Height`. | HIGH |
| **L1e** | L0 | each editor line's summed cell widths ≤ editor content width | Cheap, from existing `Cells` (sum `c.Width`); needs editor content width in Snapshot. Localizes overflow to the editor pane. | MEDIUM |
| **P1** | L0 | `View()` is pure (idempotent) | Driver renders `View().Content` twice per check; assert equal (CLAUDE.md §8.1). Reuse the L1 render to avoid a third call. | MEDIUM |
| **S1** | L0 | every `Selected` cell's `BufOffset` lies inside some cursor selection range | Derive ranges from the new `Cursors` snapshot field (`cursor.SelectionRange()`); no spurious/stale highlight. | HIGH |
| **R8** | L0 | no cell has `Rune=='\n'` or `'\r'` | `cell_test.go:16–31` invariant; `cell.go:48–49,78–79` skip newlines. Pure `Cells` check. | HIGH |
| **R9** | L0 | a cell with `BufOffset==-1` is never `Selected` or `Cursor` | Decorative cells must not carry selection/cursor (`cell.go` overlays). Pure `Cells` check. | HIGH |

---

## Family 2 — Editor-core structure (cheap, deep correctness)

These verify the editor's internal contracts that render bugs only *symptomatically* expose. All are
pure functions of `[]cursor.Cursor` / `buffer.Buffer` / `display.DisplaySnapshot` — accessors already
exist (`FuzzCursors()`, `FuzzSnapshot()`), just unused by the invariant package.

| ID | Level | Checks | Grounding | Sev |
|----|-------|--------|-----------|-----|
| **C1** | L0 | `CursorSet` sorted by `SelectionStart`, non-overlapping (`cursors[i].SelectionEnd() ≤ cursors[i+1].SelectionStart()`) | The `Merge()` contract (`cursor.go:175–248`). Multi-cursor merge bugs are invisible to R1–R7. | HIGH |
| **C2** | L0 | cursor IDs unique and `> 0` | `cursor.go:79–106`. | HIGH |
| **C3** | L0 | **both** `Position` and `Anchor` ∈ `[0, len(Content)]` | M2 checks only positions (`CursorOffsets`); unchecked anchors hide selection-bounds bugs. | HIGH |
| **B1** | L0 | `LineCount() == strings.Count(Content,"\n")+1`; `LineStart(0)==0`; `LineStart(n)` strictly increasing & ≤ len; at rune boundaries | Line-index integrity (`lineindex.go:5–13`), computable from `Content` + public `LineCount`/`LineStart`. | HIGH |
| **B2** | L1 | `next.BufferVersion ≥ prev.BufferVersion`; version strictly increases when `Content` changes | Version monotonicity (`buffer.go:172`, `Version()` public). | MEDIUM |
| **D1** | L0 | for each `State==Rendered` span with non-nil `CellMap`: `len(CellMap)==utf8.RuneCountInString(Text)` | Pre-empts the flagged `sliceOriginalSpans` CellMap OOB (`wrap_map.go:362–368`) — localizes it before the panic. Needs `Display` in Snapshot. | HIGH |
| **D2** | L0 | a `Rendered` span with non-empty `Text` must not have a nil `CellMap` | The flagged nil-CellMap → revealed-fallback surface (`cell.go:32–33`). | MEDIUM |
| **D3** | L0 | every span: `BufferStart ≤ BufferEnd`; `DisplaySnapshot` row maps consistent (`TotalRows==len(Lines)`, map lengths) | `snapshot.go:87–244`. | MEDIUM |
| **WRAP-RT** | L0 | per model line, concatenated `WrapSegment.Spans[*].Text` across its wrap rows equals the source syntax-line text | **Metamorphic oracle** — display-layer analogue of SHADOW: proves wrapping never drops/duplicates/reorders content. Localizes the flagged `sliceOriginalSpans` corruption (`wrap_map.go:193–314,362–368`). Needs `Wrap`+`Syntax` in Snapshot. | HARD STOP |
| **SPAN-COVER** | L0 | per line, span `[BufferStart,BufferEnd)` ranges tile `[lineStart, lineStart+len]` with no gap and no overlap | Gap = unrendered buffer content (silent hide); overlap = double-render (`syntax_map.go:393–526`). | HIGH |
| **COORD-RT** | L0 | `WrapToSyntax(SyntaxToWrap(p))≈p` and `SyntaxToBuffer(BufferToSyntax(b))≈b` (boundary clamping tolerated) | Cursor-mapping bijection (`wrap_map.go:25–92`, `syntax_map.go:128–192`); R1–R7 see drift only symptomatically. | HIGH |
| **D4** | L0 | within a render row, non-`-1` `BufOffset` is monotonically non-decreasing; positive `BufOffset`s are unique | No backward jumps; no rune rendered to two cells (`cell.go:28–101`, `cellmap.go:5–27`). | HIGH |
| **D5** | L0 | a `Revealed` span's `Text` equals raw buffer bytes `lineText[Start-ls : End-ls]`, byte-for-byte | Revealed spans carry source 1:1 (`syntax_map.go:428–445`). | HIGH |
| **D6** | L0 | `len(Lines)==TotalRows`; `RowToModelLine(row)` over `[0,TotalRows)` forms contiguous ranges with every value ∈ `[0,LineCount)`; `ModelLineToFirstRow(line)` over `[0,LineCount)` strictly increasing | Row↔line map integrity via **public accessors** (`snapshot.go:231–243`) — `rowToModelLine`/`lineToFirstRow` are unexported, but `len(Lines)==TotalRows` holds (`BuildSnapshot` `snapshot.go:94–139`: one `DisplayLine` per wrap row). Survives table/image row expansion (`table_rows.go`, `image_rows.go`). | HIGH |
| **B3** | L0 | every `AppliedEdit` has `0 ≤ Start ≤ End ≤ newBuf.Len()`; `len(Deleted)==oldEnd-oldStart` | Edit-metadata validity (`buffer.go:140–162`). | HIGH |
| **B4** | L0 | edits handed to `ApplyEdits` are strictly descending by `Start` | Batch-shift precondition the multi-cursor math depends on (`commands_edit_lines.go:34–40`). | HIGH |
| **C4** | L0 | `Merge()∘Merge()==Merge()`; merge keeps the min surviving ID | **Idempotence** of the merge contract (`cursor.go:175–247`). | HIGH |
| **C5** | L0 | collapse-to-position is idempotent and preserves `ID`/`DesiredCol` | Selection collapse semantics (`cursor.go:45–71`). | MEDIUM |

---

## Family 3 — Stateful / transition / liveness (the addendum's L1/L2, not yet built)

Implements `CheckTransition(prev,msg,next)` + `Monitor` automata per the addendum, threads prev/next in
the driver, and adds the spec-as-test guard invariants. **G1, TR-dirty-set, F1 are expected to FAIL
initially** — they are executable specs for features that are scaffolded but inert (`MarkDirty` never
called; `ConfirmQuitMsg` quits unconditionally at `workspace.go:1026`; `SetGuard` is dead code). That
RED result *is the point*: it proves the engine catches real missing features, then drives wiring them.

| ID | Level | Trigger (prev + msg) | Expected (next) | Sev |
|----|-------|----------------------|-----------------|-----|
| **G1** | L1 | `prev.HasDirtyFile` && `msg` is `footer.ConfirmQuitMsg` | `next.GuardVisible` | HIGH (RED today) |
| **G2** | L1 | `msg` is `footer.DataLossGuardResponseMsg` | `!next.GuardVisible` | HIGH |
| **G3** | L1 | `msg` is `footer.DataLossGuardResponseMsg{Response: footer.DataLossSave}` | `!next.HasDirtyFile` | MEDIUM |
| **TR-dirty-set** | L1 | editing keystroke && `Content` changed | `next.HasDirtyFile` | HIGH (RED today) |
| **TR-dirty-clear** | L1 | `msg` is `FileSavedMsg` | active tab not dirty | MEDIUM |
| **TR-focus-valid** | L0 | always | `FocusPane ∈ [paneTree..paneChat]` | HIGH |
| **TR-tab-opened** | L1 | `msg` is `FileLoadedMsg{Path:p}`, `p!=""` | `p` ∈ `next.Tabs` paths | HIGH |
| **F1** (monitor) | L2 | `ConfirmQuitMsg` not guard-blocked | `AppQuitting` within N steps | HIGH |
| **F2** (monitor) | L2 | `FileLoadedMsg{p}` | `p` appears in tabs within N steps | MEDIUM |
| **F3** (monitor) | L2 | `activeSave.InFlight` set | clears within N steps (no stuck save) | MEDIUM |
| **RESIZE-INV** | L1 | `msg` is `tea.WindowSizeMsg` | `next.Content==prev.Content`; cursor offsets & `HasDirtyFile` unchanged (resize is layout-only, never mutates the buffer) | HIGH |
| **EDITOR-TAB-COH** | L0 | always | `EditorPath == Tabs[ActiveTabIdx].Path` and `(DocID>0)==(EditorPath!="")` — editor's open file equals the active tab (`workspace.go:853,859–862,1367`) | HIGH |
| **SAVE-NOMUT** | L1 | `msg` is `FileSavedMsg` | `Content` unchanged across the save; `activeSave.SavedContent` == content captured at `startSave` (`workspace.go:357–363`) | HIGH |
| **TAB-SET** | L0 | always | exactly one tab `Active` when non-empty and `ActiveTabIdx` == that index; all tab paths unique (`opentabs.go:67,69,86,99`) | HIGH |
| **FT-BOUNDS** | L0 | always | filetree `cursor ∈ [0, len(entries))` (or 0 when empty) (`filetree.go:34–35,130–168`) | HIGH |
| **GUARD-SYNC** | L0 | always | `GuardOptionCount>0 ⟺ GuardKind` is a valid enum; both clear atomically on response (`footer.go:101–107,122–141`) | HIGH |
| **SAVE-SM** | L0 | always | ≤1 `activeSave.InFlight`; `FlushGen` monotonic non-decreasing (`workspace.go:354–364,464–482`) | MEDIUM |
| **F4** (monitor) | L2 | `ChordPending` set | clears within N steps if no completing key arrives (chord timeout) | MEDIUM |
| **F5** (monitor) | L2 | always | cursor / open-tab / store-snapshot counts stay bounded relative to issued ops (no leak/duplication) | MEDIUM |

---

## Cross-cutting — Generation & seeding efficiency

Invariants only help if the fuzzer *reaches* the states. Today `FuzzSession` has 9 seeds and key events
index *modulo* into the binding table (`driver/keytable.go`), so undo/redo/save/find/multi-cursor
and the dirty→quit path are hit rarely by chance — and, per Context facts 4–5, *no* path types markdown
or assembles multi-char content cheaply. Add:

0. **Generation fixes (prerequisite — Family-0 priority).** Two harness defects gate every content-heavy
   and render invariant; fix them first or the display family checks dead code:
   - **`KindText` full-string insert** (`keytable.go:74–80`): expand the first-rune-only handler to insert
     the whole `ev.Text` (loop runes → multiple `KeyPressMsg`, or one multi-rune insert). Without this,
     long-line / multi-line / markdown states are O(events) and the mutator rarely builds them.
   - **Markdown metachars in `bindingTable`** (`keytable.go:55–60`): add a `* # | [ ] ! _ - > ` ( ) ~`
     block so `KindKey` can type markdown tokens. Until then, Rendered/Revealed spans, table-row and
     image-row expansion (`syntax_map.go`, `table_rows.go`, `image_rows.go`) are reachable only via
     `KindPaste` — see the markdown cluster below.
1. **Command-coverage seeds** (`f.Add` in `session_fuzz_test.go`): undo/redo round-trip, ⌘S save,
   find, multi-cursor add, large paste, tab open/close/pin cycle, and the guard path
   `[insert, ^C, ^C]` (the addendum's G1 seed). Each new invariant ships with its minimal seed. New
   clusters that reach the *new* precondition states (bulk text via `KindPaste`):
   - **Markdown-syntax** — `KindPaste("# H\n**b** _i_ `c`\n| a | b |\n|---|---|\n| 1 | 2 |\n![x](y.png)")`.
     The *only* way to exercise Rendered/Revealed spans, table-row and image-row expansion. Drives
     SPAN-COVER, D4–D6, WRAP-RT. **Highest priority** — pairs with the metachar fix (0).
   - **Wrap-stress** — paste a line wider than the terminal, `KindResize(20,h)` to force soft-wrap
     (exercises `sliceOriginalSpans` + CellMap slicing — the flagged bug), then `KindResize(200,h)` back.
     The single best seed for WRAP-RT / COORD-RT through the real wrap path.
   - **Multi-line scaffold + line ops** — paste `"a\nb\nc\nd"`, then `MoveLineUp/Down`, `AddCursorBelow`×2,
     then edit. Today every seed is effectively single-line (fact 4), so line-manipulation and add-cursor
     are no-ops (`AddCursorBelow` needs line<last; `MoveLineUp` needs line>0).
   - **Multi-cursor paste-distribution** — scaffold N lines, add a cursor per line, copy a multi-line
     selection, paste → hits the `len(lines)==len(cursors)` distribution branch (`commands_clipboard.go:220–225`).
   - **Selection-replace + undo** — `SelectAll` (or shift-moves) → type/paste over the selection → undo;
     exercises the replace-selection edit path and its restoration (DL4).
   - **Cross-surface undo** — `ctrl+r` (chat) → edit → `ctrl+e` (editor) → edit → `ctrl+z`×2 / `ctrl+y`×2.
     The journal is per-surface (main/title/chat) with focus-follows-surface; the fuzzer only edits `main`.
   - **Dirty multi-tab guard** — `ctrl+n` → type → `ctrl+n` → type → ⌘S (one tab) → `^C ^C`. Reaches G1
     with the *correct* tab dirty alongside a clean one — stronger than the single-buffer `[insert,^C,^C]`.
   - **Undo-truncates-future** — type → `⌘Z` → type-different → `ctrl+y`; asserts redo is a no-op (DL5)
     and exercises the `journal.go:28–34` future-truncation in `(*Store).AppendEdit`.
   - **Autosave/snapshot** — type → (flush fires; `flushDelay=0` so `CreateSnapshot` runs as a drained
     Cmd) → type → ⌘S; drives DL2 and the `FlushGen` monotonicity check (SAVE-SM).
2. **Binding-coverage corpus**: one seed enumerating `KeyIndex 0..N-1` so every binding fires at least
   once (precursor to the Phase-2 decode-smoke pass).
3. **Snapshot-aware weighting** (Phase 1+, addendum §seeding-3): a `WeightedEventGen` that biases toward
   quit keys when `snap.HasDirtyFile`, toward tab/focus keys when many tabs exist, and toward `KindPaste`
   of structured/markdown text when the buffer is empty or short (uniform `KindKey` mutation almost never
   assembles a markdown token or a wrap-width line on its own) — to reach precondition-gated invariants
   (G1, SPAN-COVER, WRAP-RT) far faster than uniform random.

---

## Harness changes

**`internal/fuzz/invariant/invariant.go`** — extend `Snapshot`:
```go
// render/layout
Frame  string ; Width, Height int ; EditorContentWidth int
Wrap   display.WrapSnapshot ; Syntax display.SyntaxSnapshot   // WRAP-RT / COORD-RT / SPAN-COVER / D5
// editor core
Cursors []cursor.Cursor ; BufferVersion uint64 ; LineCount int
Display display.DisplaySnapshot
// orchestration coherence
EditorPath string ; TabActive []bool                     // TAB-SET (Tabs/ActiveTabIdx already present)
FiletreeCursor, FiletreeLen int                           // FT-BOUNDS
SaveSnapshot []byte ; FlushGen uint64 ; GuardOptionCount int  // SAVE-NOMUT / SAVE-SM / GUARD-SYNC
// transition/guard/liveness  (addendum)
HasDirtyFile, GuardVisible, ChordPending, AppQuitting, SaveInFlight bool
GuardKind footer.GuardKind ; FocusPane int ; DocID int64
```
**B3/B4** verify the edit-application pipeline, not a settled Snapshot — expose them via a gated
`FuzzLastApplied() []buffer.AppliedEdit` (and the descending-order assertion at the call site in
`commands_edit_lines.go`) rather than threading per-edit data through `Snapshot`.
Add `CheckTransition(prev Snapshot, msg tea.Msg, next Snapshot) []Violation` and `monitor.go`
(`Monitor` iface: `Observe(prev,msg,next) []Violation` + `Reset()`). Invariant pkg may import
`cursor`, `buffer`, `display`, `footer` (all leaf-ward of `invariant`; no cycle — it never imports
`workspace`).

**Gated accessors** (`//go:build fuzzing`, value receivers, read-only):
- `workspace_fuzz.go`: extend `FuzzInspect()` to populate the new fields — `FocusPane` from `m.focus`;
  `HasDirtyFile` by OR-ing `Tab.Dirty` over the new gated `opentabs.FuzzTabs()`; `GuardVisible`/`GuardKind` from `m.footer.InGuard()`/
  `GuardKind()`; `ChordPending` from new `footer.PendingKey()`; `SaveInFlight` from `m.activeSave.InFlight`;
  `DocID` from `m.docID`; `BufferVersion`/`LineCount`/`Cursors`/`Display`/`Wrap`/`Syntax` forwarded from
  the editor; `EditorPath` from `m.filePath`; `TabActive` from the `Tab.Active` flags of that same `opentabs.FuzzTabs()`; `FiletreeCursor`/
  `FiletreeLen` from the filetree; `FlushGen` from `m.flushGen`; `SaveSnapshot` from `m.activeSave.SavedContent`;
  `GuardOptionCount` from new `footer.GuardOptionCount()`.
- `textedit_fuzz.go`/`markdownedit_fuzz.go`: add `FuzzBufferVersion()`, `FuzzLineCount()` (or
  `FuzzBuffer() buffer.Buffer`), `FuzzWrapSnapshot() display.WrapSnapshot` (returns `m.wrapSnap`, `textedit.go:58`)
  / `FuzzSyntaxSnapshot()` for WRAP-RT/COORD-RT/SPAN-COVER,
  and `FuzzLastApplied() []buffer.AppliedEdit` for B3/B4; `FuzzCursors()` and `FuzzSnapshot()` already exist.
- `footer.go`: add `PendingKey() string` and `GuardOptionCount() int` accessors (`pendingKey`/`guardOptions`
  are private; `InGuard()`/`GuardKind()` exist).
- `opentabs.go`: add `FuzzTabs() []Tab` — returns the tab slice (incl. `Active`/`Dirty`/`Path`). `tabs` is
  private (`opentabs.go:28`) and no exported method exposes active/dirty flags. This one accessor sources
  **both** `TabActive` (TAB-SET) and `HasDirtyFile`; `FuzzInspect` reduces it to `TabActive []bool` +
  `HasDirtyFile bool`, so the `invariant` package never imports `opentabs`.
- `driver/keytable.go`: apply the generation fixes (`KindText` full-string insert; markdown metachars in
  `bindingTable`) — see Cross-cutting item 0.

**Driver (`driver.go`)** — thread prev/next and the mirror in `drainMsg`:
```go
prev := m.FuzzInspect()
m2, cmd := m.Update(msg)
next := m2.FuzzInspect()
next.MirrorContent = replay("", store.AllEdits("main"))  // SHADOW: fold each batch ascending by Start
if isQuit(msg) { next.AppQuitting = true }             // F1
violations := append(CheckInvariants(next), CheckTransition(prev, msg, next)...)
for _, mon := range monitors { violations = append(violations, mon.Observe(prev,msg,next)...) }
if isSaved(msg) { violations = append(violations, CheckDataLossInvariants(next)...) }  // DL1
```
Instantiate monitors per `Run` (fresh state for each shrink replay); detect `tea.QuitMsg`/
`FileSavedMsg` by type. `CheckInvariants` returns a single `*Violation` today — either keep first-wins
or migrate to `[]Violation`; recommend keeping first-wins (`CheckTransition`/monitors append, driver
freezes on the first non-nil).

**`pkg/docstate`** (additive, non-gated): `(*Store) AllEdits(surface string) ([][]buffer.AppliedEdit, error)`
— **batch-grouped** (each `AppendEdit` row = one batch; reuses the journal JSON codec). The driver's
`replay(initial string, batches [][]buffer.AppliedEdit) string` folds each batch in **ascending-`Start`
order** (sort ascending, or iterate the descending `applied` array in reverse) so each edit's post-shift
`Start` aligns with the running mirror — the naive flat fold breaks multi-cursor batches.
`LatestSnapshot`/`CreateSnapshot` already exist for DL2.

---

## Sequencing

0. **Unblock generation (Family-0 priority)** — fix `KindText` full-string insert + add markdown metachars
   to `bindingTable`; land the markdown and wrap-stress seeds. Prerequisite: without it the display family
   (Family 1/2 below) only ever checks plain text and the Rendered path stays dead.
1. **Deepen (Family 0)** — wire SHADOW (`AllEdits` + mirror replay), thread DL1, add DL2; then DL4
   (undo-stack RT), DL5 (undo-truncate), DL6 (coalesce-bound). Converts the primary oracle from no-op to
   live. Lowest effort, highest payoff.
2. **Cheap L0 widen** — R8, R9, B1, B3, B4, C1–C5, L1, L2 (Snapshot grows `Frame`/`Width`/`Height`/`Cursors`).
3. **Display L0** — D1–D6, SPAN-COVER, then the metamorphic oracles WRAP-RT, COORD-RT (add
   `Display`/`Wrap`/`Syntax` to Snapshot via `FuzzSnapshot()`/new accessors).
4. **Transition framework (Family 3)** — `CheckTransition` + driver prev/next; TR-focus-valid,
   TR-tab-opened, EDITOR-TAB-COH, TAB-SET, FT-BOUNDS, GUARD-SYNC, SAVE-SM, RESIZE-INV, SAVE-NOMUT, then
   G1/TR-dirty-set/TR-dirty-clear as failing specs.
5. **Monitors + DL3** — F1–F5, undo/redo non-loss.
6. **Generation/seeding** — remaining command-coverage clusters (cross-surface undo, multi-cursor
   distribution, dirty multi-tab guard, …) + binding sweep, then snapshot-aware weighting.

---

## Verification

- **Both builds compile:** `go build ./...` (no fuzzing symbols) and `go build -tags fuzzing ./...`.
- **SHADOW now fires:** unit-test the driver on a seed that edits; temporarily corrupt the mirror replay
  and assert SHADOW trips (proves it is no longer inert). Run a planted buffer bug → caught + minimized.
- **SHADOW multi-cursor replay is correct:** unit-test `replay` on a single batch with two non-adjacent
  edits of differing net length (e.g. delete 2 + insert 1 at the low offset, delete 1 + insert 3 at the
  high offset); assert it reconstructs `buffer.Content()`, and assert the naive flat/descending fold
  produces the WRONG string — a regression guard against reverting to `mirror[:e.Start]+…` in array order.
- **Per-invariant unit tests** (`go test -tags fuzzing ./internal/fuzz/invariant/`): hand-build a
  Snapshot violating each new L0 check (overflow line for L1, `\n` cell for R8, unsorted cursors for C1,
  bad `LineCount` for B1, short CellMap for D1, a gap in the per-line span tiling for SPAN-COVER, a
  mismatched wrap-segment for WRAP-RT, a backward `BufOffset` for D4, a stale-active-tab for EDITOR-TAB-COH,
  two active tabs for TAB-SET) and assert it fires; assert a clean Snapshot is green.
- **Generation fixes land:** assert the `KindText` "hello" seed now inserts `hello` (not `h`); assert the
  markdown-paste seed produces ≥1 `State==Rendered` span in `FuzzSnapshot()` — proving the Rendered path
  is finally reachable. Run the wrap-stress seed; assert WRAP-RT/COORD-RT actually execute against wrapped
  rows (TotalRows > model line count).
- **RESIZE-INV:** drive `[paste content, KindResize…]`; assert `Content`/cursor offsets/`HasDirtyFile`
  equal across the resize; plant a content-mutating resize bug and assert it trips.
- **CheckTransition:** feed `(dirty prev, ConfirmQuitMsg, no-guard next)`; assert **G1 fires** (RED by
  design). After wiring `SetGuard` on dirty-quit + `MarkDirty` on edit + `MarkClean` on save, re-run the
  `[insert,^C,^C]` seed and assert **G1/TR-dirty-set green** (invariant-as-spec drove the feature).
- **Monitors reset:** run the shrinker on an F1 repro twice; assert identical violation (deterministic).
- **DL-family:** Phase-2 `OpenAt(t.TempDir())` seed that saves; assert DL1/DL2 green; corrupt the
  on-disk file in a test and assert DL1 trips. Undo/redo seed → assert DL3 round-trip equality.
- **Engine:** `go test -tags fuzzing -fuzz=FuzzSession ./pkg/ui/pages/workspace -run=^$ -fuzztime=…`;
  confirm a deliberately-broken EOL synthetic-cursor cell is caught by R6, and a broken `sliceOriginalSpans`
  by D1, with shrunk `keys.jsonl`.
- **Determinism/perf:** replay a `keys.jsonl` N times → identical Violation + frame; confirm adding
  `View()`-per-check (L1/L2/P1) does not regress drain quiescence.

## Files to modify

- `internal/fuzz/invariant/invariant.go` (Snapshot, new L0 checks, `CheckTransition`), new
  `internal/fuzz/invariant/monitor.go`.
- `internal/fuzz/driver/driver.go` (prev/next, mirror, DL1/monitors threading);
  `internal/fuzz/driver/keytable.go` (**generation fixes**: `KindText` full-string insert + markdown
  metachars in `bindingTable`).
- `pkg/ui/pages/workspace/workspace_fuzz.go` (extend `FuzzInspect` — `EditorPath`, `TabActive`, `HasDirtyFile`,
  `DocID`, filetree cursor/len, `FlushGen`, `SaveSnapshot`, `GuardOptionCount`, `Wrap`/`Syntax`); `textedit_fuzz.go` /
  `markdownedit_fuzz.go` (buffer/version/linecount/`FuzzWrapSnapshot`/syntax/`FuzzLastApplied` accessors);
  `pkg/ui/components/footer/footer.go` (`PendingKey()`, `GuardOptionCount()`);
  `pkg/ui/components/opentabs/opentabs.go` (gated `FuzzTabs() []Tab`).
- `pkg/docstate/journal.go` (`AllEdits(surface) ([][]buffer.AppliedEdit, error)` — batch-grouped).
- `pkg/ui/pages/workspace/session_fuzz_test.go` (new seeds).
- *(Family-3 feature wiring, if landing specs green: `workspace.go` `ConfirmQuitMsg`/`journalEdit`/
  `FileSavedMsg` handlers to call `footer.SetGuard` / `opentabs.MarkDirty` / `MarkClean`.)*
