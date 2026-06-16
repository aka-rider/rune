# Rune Fuzzing Harness — Implementation Plan

## Context
`rune` is a Bubble Tea v2 TUI markdown editor. We want a fuzzer that penetrates deep
TUI/editor states, checks invariants continuously, and on the first violation captures a
**reproducible** artifact bundle — including a real **Playwright screenshot** of the frozen
terminal — for LLM triage. The design below was settled branch-by-branch (see Decision Log).

Key shape: a **fast, deterministic, in-process engine** does discovery + minimization (no
browser); **Playwright is invoked per-finding only**, to screenshot the frozen frame of a
minimal repro replayed through ttyd. No sleep-based logic anywhere; races are explicitly out
of scope (un-reproducible from a static journal).

## Grounded facts (from codebase investigation)
- **Buffer** (`pkg/editor/buffer/buffer.go`): immutable `string` + `lineStarts []int` +
  `version uint64`; UTF-8 validated at edit boundaries; value semantics; `AppliedEdit`
  {Start,End,Deleted,Insert} records each mutation.
- **Cursor** (`pkg/editor/cursor/cursor.go`): byte-offset Position/Anchor/DesiredCol/ID;
  multi-cursor `CursorSet.Merge()` => sorted, non-overlapping.
- **Coords** (`pkg/editor/coords/`): Buffer→BufferPoint→SyntaxPoint→WrapPoint→DisplayPoint;
  round-trips are `*testing.T`-checked (`TestCoordinateTypes`), NOT `testing.F`-fuzzed.
- **Render** (`pkg/editor/display/` + `pkg/ui/components/textedit/cell.go`): goldmark parse →
  `SyntaxSpan` (RevealState Rendered/Revealed, `CellMap`) → wrap → `DisplaySnapshot` →
  `[]Cell` (Rune/Width/BufOffset/Cursor/Selected) → ANSI. Reveal = raw md on cursor line,
  rendered off-line. Flagged bug surfaces: EOL synthetic-cursor cell (double cursor),
  `sliceOriginalSpans` CellMap slice OOB, nil-CellMap fallback for Rendered spans.
- **Input** (`pkg/ui/pages/workspace/workspace.go`, `pkg/editor/keybind/`): modeless;
  keys→resolver→command→`registry.Execute`→`Operation`. ~90 physical key seqs, ~60 commands.
  5 focus panes (tree/tabs/center/title/chat). 5-layer priority routing in `workspace.Update`.
  `Update` batches via `tea.Batch` (EXPORTED `BatchMsg []Cmd`) and `tea.Sequence` (UNEXPORTED
  `sequenceMsg []Cmd`, used at workspace.go:1024). `View()` returns `tea.View` — the frame is at
  `.Content`, NOT a bare `string`.
- **Async cascade**: a keystroke → `edit.insert-character` → `scheduleFlush()` (2s
  `time.Sleep` Cmd) → `pendingFlushMsg` → `CreateSnapshot()` + `startSave()` → `saveFileCmd`
  → `FileSavedMsg`. State settles over an async tail, not on the synchronous keystroke.
- **Self-rescheduling / blocking Cmds** (MUST be neutralized under fuzzing or the inline drain
  never quiesces): `watchDirCmd` infinite `for{select}` (workspace.go:1277-1324, fired by
  Init→`DirLoadedMsg`→`startWatch`); image `scheduleFrame` `tea.Tick` METHOD (image.go:227-232, armed
  by `ArmTick` on SetContent/layout); one-shot timers — footer error-dismiss 5s (footer.go:191),
  footer confirm-exit 2s (footer.go:205), `scheduleFlush` 2s (workspace.go:484); dictation mic
  goroutine via `StartCmd` (dictation.go:71, only on the toggle key — `dict.Init()` is `nil`).
- **Persistence** (`pkg/docstate/`): SQLite `perm` HARDCODED at `$HOME/.local/share/rune/
  rune.db` (Open() reads `os.Getenv("HOME")`; falls back to `:memory:`). `mem` `:memory:`
  journal (events). `Store.clock func() time.Time` is INJECTABLE. Undo coalescing window 300ms
  (clock-driven). File save via `os.WriteFile` 0644 on ⌘S / autosave.
- **Tests**: Go-native `testing.F` for **buffer/display** roundtrips ONLY; cursor/coords have
  `*testing.T` property tests (`TestCursorSetBoundsFuzzer`, `TestCoordinateTypes`), NOT `testing.F`. Helpers in
  `internal/editortest/` (ParseState/FormatState = STATE notation, Clock value type, golden,
  diff). NO Model.Update integration harness, NO keystroke-sequence replay, NO teatest, NO
  logging, NO `--fuzzing` flag/tag — all NEW.
- **Entry** `cmd/rune/main.go`: flag `-w <dir>`, up to 10 positional files. orqestra = external.

## Architecture (resolved)
```
DISCOVER + SHRINK  (in-process, deterministic, NO browser, CI-able)
  go test -fuzz=FuzzSession              <- coverage-guided engine (penetration + min)
    bytes --decode--> []Event            <- stable decoder
    initial workspace.Model
    for each Event:  model,cmd = Update(ev);  drain ALL cmds inline (timers collapsed)
                     after every settled Msg: CheckInvariants(model)
    on violation: testing reports; libFuzzer writes failing corpus entry
  delta-debug shrinker (event-level) --> minimal keys.jsonl (human/LLM readable)

PER FINDING  (browser, once)
  rune-fuzz --fuzz-script=minimal keys.jsonl   (built -tags fuzzing)
    synchronous driver replays; on FIRST violation: FREEZE frame, render sentinel
      <<RUNE_FUZZ_VIOLATION id=..>>, dump artifact bundle, block (no exit)
  ttyd --writable serves rune-fuzz ; Playwright busy-polls buffer for sentinel
      (browser_evaluate reads, NEVER a timed wait) -> browser screenshot -> archive
  fresh process per session (in-memory store; OpenAt(tempDir) only for Phase 2 DATA-LOSS)

DECODE SMOKE  (browser, once total)
  press each of ~90 bindings through ttyd -> assert resolver mapped key->command
```

## Components to build
- `internal/fuzz/event/` — `Event` type {Kind: key|text|paste|resize|focus|forceViolation, ...};
  **binary length-prefixed bytes→[]Event decoder** (the fuzz input format) + `events ↔ JSONL`
  converters (JSONL is ONLY the human/LLM-readable repro/seed format, never the fuzz input).
  The binary decoder is **total** — every byte string decodes to some event list, so libFuzzer
  never wastes inputs and a small byte flip → a small event change (byte-shrink ≈ event-shrink):
  - Frame = 1 discriminant byte `kind`, then a per-kind payload.
  - `kind`: `0=key, 1=text, 2=paste, 3=resize, 4=focus, 5=forceViolation` (5 = Phase-0 plumbing).
  - `key`: 1–2 payload bytes indexed (modulo) into the ~90-binding table → always in range.
  - `text`/`paste`: 1 length byte + N bytes; non-UTF-8 sanitized (invalid runes dropped).
  - `resize`: 2 bytes (w,h) clamped to a sane terminal range.
  - `focus`: 1 byte `% 5` (pane index).
- `internal/fuzz/driver/` — synchronous deterministic driver: steps Events through
  `model.Update`, **drains every returned Cmd inline** until quiescent. Cmd-slice messages are
  recognized by reflection (`asCmdSlice`): exported `tea.BatchMsg` fast-path, else any msg whose
  underlying type is `[]tea.Cmd` — the ONLY way to catch the UNEXPORTED `sequenceMsg`. Drains in
  order (valid for both Batch and Sequence): run Cmd → feed Msg to `Update` → drain nested → next.
  Timers collapsed (gated 0-delay vars); self-rescheduling Cmds stubbed (see refactors). Frame
  captured as `model.View().Content` (View returns `tea.View`). Bootstrap order: construct →
  `WindowSizeMsg`+drain → `Init()`+drain (stubs prevent the watch/tick deadlock) → inject
  `StoreReadyMsg`+drain → THEN feed Events; invokes `CheckInvariants(model.FuzzInspect())` after
  each settled Msg. Owns the loop — `pkg/ui/app.go` is NOT modified.
- `internal/fuzz/invariant/` — `CheckInvariants(s Snapshot) []Violation` + `Snapshot`/`Violation`
  types. `Snapshot` is a flat value fed by the gated `workspace.FuzzInspect()` — the editor and its
  buffer/cursors/cells are UNEXPORTED, so the engine CANNOT reach them without a gated accessor.
  This package never imports `workspace` → no import cycle. RENDER R1–R7 on `[]Cell`/
  `DisplaySnapshot`; SHADOW oracle (re-apply recorded `[]AppliedEdit` to an independent string
  mirror; compare `buffer.Content()` + cursor offsets); MODEL re-checks (buffer/cursor/coords);
  DATA-LOSS (file-on-disk==buffer, snapshot blob round-trip, undo/redo non-loss) — HARD STOP severity.
  TAB BAR invariants (T1–T2):
  - **T1 (no duplicate paths):** `tabs[i].Path == tabs[j].Path` for any `i != j` is invalid. The
    tab bar MUST NOT contain two entries for the same file path simultaneously. The invalid UI state
    `[Untitled 2.md, Untitled 2.md]` is the canonical example.
  - **T2 (active tab in set):** if `activeIndex >= 0`, `tabs[activeIndex]` MUST exist;
    `activeIndex` MUST NOT reference an out-of-range slot.
- `internal/fuzz/artifact/` — writes the per-finding bundle (see Decision Log D11).
- `internal/fuzz/shrink/` — event-level delta-debug; re-runs script-minus-events through the
  driver, keeps reductions that reproduce the same `Violation.invariant_id`.
- `pkg/ui/pages/workspace/session_fuzz_test.go` (`//go:build fuzzing`) — `FuzzSession(f)`: build
  workspace model, decode bytes→events, drive, assert zero violations; `f.Add` LLM seed corpus.
- **Gated inspection surface** (`//go:build fuzzing`, value receivers, read-only §5.1):
  `workspace_fuzz.go` (`FuzzInspect()`); `textedit_fuzz.go` (`FuzzCells`/`FuzzSnapshot`/
  `FuzzCursors`) forwarded from `markdownedit_fuzz.go`. Rationale: the driver/invariant engine is
  reusable across `FuzzSession` AND `run_fuzz.go`, so it needs a gated PUBLIC surface — an in-package
  test alone can't serve the binary.
  - **`FuzzCells` cannot read a field** — there is NO stored `[]Cell`; cells are built in
    `textedit.View()` (textedit.go:835-926: `Snapshot().Slice()` → `SpanToCells(sp, base)` per span
    → `SliceCells` → `ApplyOverlays`). Extract that loop into an unexported helper
    `(m Model) renderCells() [][]Cell`; `View()` AND the gated `FuzzCells()` both call it, so the
    fuzzer checks EXACTLY what renders (no drift). `Cell` also carries `Grapheme`/`Style` beyond
    Rune/Width/BufOffset/Cursor/Selected. Reuse already-exported `Snapshot()` (textedit.go:563),
    `CursorOffsets()` (:386), `Selections()` (:395).
  - **NO `FuzzRecordedEdits`** — `DrainEdits()` is already public on `textedit.Model`
    (textedit.go:438) and `markdownedit.Model` (markdownedit.go:93), returning
    `(Model, []buffer.AppliedEdit)`. The driver calls it directly in its outer loop, threading the
    returned model (it clears `pendingEdits`), accumulating edits for the SHADOW oracle.
- `cmd/rune/run.go` (`//go:build !fuzzing`) + `cmd/rune/run_fuzz.go` (`//go:build fuzzing`):
  production runs stock `tea.Program`; fuzzing runs the synchronous driver against pty input
  + `--fuzz-script`, sentinel render, artifact dump, injected store (`OpenInMemory`/`OpenAt`) + clock.
  **Sentinel (G6):** `run_fuzz.go` owns the output loop (writes frames to the pty), so on first
  violation it FREEZES and emits its OWN composed frame = `<<RUNE_FUZZ_VIOLATION id=...>>` +
  the frozen `model.View().Content`, then blocks. `workspace.View()` is NOT modified (single frame
  point is workspace.go:1161/1164 via `tea.NewView`; `tea.View.Content` is a settable string).
  Playwright busy-polls the ttyd buffer for the sentinel substring via `browser_evaluate`. Events
  become input via directly-constructed `tea.KeyPressMsg{...}` (no exported tea-v2 pty-byte decoder).

## Required refactors to existing code (small; gated files are `//go:build fuzzing`)
- `pkg/docstate` (additive, non-gated): add **`OpenInMemory(clock func() time.Time)`** (both `perm`
  and `mem` `:memory:` — Phase 0/1 isolation with NO disk and NO path threading) and
  **`OpenAt(baseDir string)`** (Phase 2 real-file isolation for DATA-LOSS). Production `Open()`
  delegates to `OpenAt(home)`. Add `(*Store).SetClock` so the driver injects a logical clock.
  Note: the docstate path ($HOME-hardcoded, store.go:138) and the **filetree working dir**
  (`-w` else `os.Getwd()`, workspace.go:131-139) are INDEPENDENT — both must be redirected.
  Phase 0/1: `OpenInMemory` + a throwaway `t.TempDir()` workdir (tree has a root, no real files,
  DATA-LOSS invariants don't run). Phase 2: `OpenAt(t.TempDir())` + a real temp workdir so
  save/load hit actual files.
- **Store injection**: gate `openStoreCmd` → returns `nil` under fuzzing (`workspace_store.go`
  `!fuzzing` / `workspace_store_fuzz.go` `fuzzing`). The driver injects by feeding the EXPORTED
  `workspace.StoreReadyMsg{Store: store}` after the `Init()` drain (exercises the real
  `StoreReadyMsg`→`EnsureDocument` path, workspace.go:904). `store` = `OpenInMemory(clock)`
  (Phase 0/1) / `OpenAt(t.TempDir())` (Phase 2); driver `Close()`s it per session (no handle leak).
- **Relocate self-rescheduling Cmds, then gate** (else the drain never quiesces; a gated shadow
  alone won't compile — the existing definition must be REMOVED first):
  - `watchDirCmd` is a standalone func with an infinite `for{select}` loop (workspace.go:1277-1324).
    MOVE it (with its `context`/`fsnotify` imports) out of `workspace.go` into `workspace_watch.go`
    (`!fuzzing`, real impl); add `workspace_watch_fuzz.go` (`fuzzing`) = `func watchDirCmd(ctx, dir)
    tea.Cmd { return nil }`. `startWatch` (workspace.go:1333) stays shared and calls it unchanged.
  - image `scheduleFrame` is a METHOD on `image.Model` using `tea.Tick` (image.go:227-232; armed at
    ArmTick:243, handleInner:162). MOVE the method out of `image.go` into `image_tick.go`
    (`!fuzzing`); add `image_tick_fuzz.go` (`fuzzing`) = `func (m Model) scheduleFrame(gen, next int,
    d time.Duration) tea.Cmd { return nil }`. `ArmTick`/`handleInner` stay shared.
- **Zero one-shot timer delays** — 3-step extraction per delay (a gated var override fails to
  compile unless the production decl is ALSO gated → duplicate symbol under `-tags fuzzing`):
  (1) extract each bare `time.Sleep(<lit>)` to a package `var` referenced in the body; (2) move the
  production `var` decl into a `!fuzzing` file; (3) add the `fuzzing` override = `0`. The three
  literals: `scheduleFlush` `time.Sleep(2*time.Second)` (workspace.go:484) → `flushDelay`
  (`workspace_timers.go`/`_fuzz.go`); footer `ShowErrorMsg` `time.Sleep(5*time.Second)`
  (footer.go:191) → `errorDismissDelay` and `startConfirmTimer` `time.Sleep(2*time.Second)`
  (footer.go:205) → `confirmDelay` (`footer_timers.go`/`_fuzz.go`). The Cmd still delivers its Msg,
  just immediately. Logical `Store.clock` gives deterministic timestamps + 300ms coalesce.
- **Dictation**: exclude the dictation-toggle binding from the Phase 0/1 event grammar (and/or stub
  `StartCmd` under fuzzing) so the mic goroutine never spawns.
- The whole engine builds/tests with **`-tags fuzzing`**; production compiles none of it (D10).

## Phased plan
### Phase 0 — Thin end-to-end slice (CHOSEN first cut)
Goal: prove the full freeze→sentinel→screenshot→artifact plumbing with minimal depth.
1. `internal/fuzz/event` minimal: key + text + `forceViolation` events; JSONL load.
2. `internal/fuzz/driver` minimal: bootstrap (construct → `WindowSizeMsg` → `Init()`+drain with the
   watch/tick stubs in place → inject `StoreReadyMsg{OpenInMemory}` → drain), THEN drive Update,
   drain Cmds inline (`asCmdSlice` handles `tea.Batch` AND the unexported `tea.Sequence`), collapse
   one-shot timers, render `View().Content`, freeze on first violation.
3. `internal/fuzz/invariant`: just **R1 (cursor-cell count == active cursor count)** +
   **SHADOW (content/cursor)**.
4. `cmd/rune/run_fuzz.go` (`-tags fuzzing`): `--fuzz-script`, sentinel render, write a minimal
   `violation.json` + `frame.txt` + `screenshot.png` slot.
5. **Forced-violation hook (gated):** the reserved `forceViolation` event makes the driver's
   `CheckInvariants` return `Violation{invariant_id:"FORCED"}` — exercising the
   freeze→sentinel→screenshot→artifact path deterministically, with zero dependence on a real bug.
   Phase-0 seed = `[text "x", forceViolation]`. Hook is `//go:build fuzzing` only; real R1 /
   EOL-double-cursor seeds arrive in Phase 1.
6. ttyd serves `rune-fuzz`; Playwright `browser_navigate` → busy-poll buffer for the sentinel
   → `browser_take_screenshot` → save into the artifact dir. Fresh process per run.
**Exit criterion:** one deliberate violation produces a frozen frame + a real screenshot +
`violation.json` on disk, fully reproducibly.

### Phase 1 — Deepen the engine
- Full event grammar (paste unicode/emoji/huge, resize, focus across 5 panes).
- `FuzzSession` Go-native target + bytes→event decoder; seed with LLM adversarial sessions.
- All RENDER R1–R7 + TAB BAR T1–T2 + MODEL re-checks; event-level delta-debug shrinker → minimal `keys.jsonl`.

### Phase 2 — Full coverage + persistence
- DATA-LOSS invariants on a real temp dir (`OpenAt`): file-on-disk==buffer after save, snapshot
  blob round-trip, undo/redo non-loss. Decode-smoke pass over ~90 bindings. Full artifact
  bundle (db.sqlite, journal.jsonl, cells.json, meta.json). Regression corpus re-run in CI.

## Risks / IMPL-VERIFY
- **tea.Batch/Sequence unwrap — RESOLVED**: `BatchMsg` is exported, `sequenceMsg` is UNEXPORTED
  (`commands.go:21,30`; both underlying `[]tea.Cmd`, both wrapped in a `func() Msg` closure). The
  driver detects slice-of-Cmd by reflection (`asCmdSlice`) and drains in order — no exported type
  name needed. Production uses `tea.Sequence` at workspace.go:1024.
- **Terminating drain — RESOLVED (enumerated)**: relocate-then-gate `watchDirCmd` (infinite loop,
  workspace.go:1277-1324) and the `scheduleFrame` METHOD (`tea.Tick`, image.go:227-232) to `nil`;
  zero the one-shot timers (footer 5s @191 / 2s @205, flush 2s @484) via the 3-step var extraction;
  exclude the dictation-toggle binding. Full list in Required refactors.
- **Refactor sequence — RESOLVED (verified file:line)**: B1 timers are bare literals (3-step
  extraction, not a bare gated var); B2 `watchDirCmd` and B3 `scheduleFrame` must be REMOVED from
  their current files before the gated split; B4 cells have no stored field (gated `FuzzCells` reuses
  an extracted `renderCells()` helper); `DrainEdits()` is already public so no `FuzzRecordedEdits`.
- **Input decode coverage**: no exported tea-v2 pty-bytes→`KeyPressMsg` decoder found, so
  `run_fuzz.go` constructs `tea.KeyPressMsg{...}` directly from decoded events; the decode-smoke
  pass (Phase 2) is the safety net for the binding table.
- **Shadow fidelity**: oracle covers content + cursor offsets only (NOT markdown render).

## Verification
- Compiles both ways: `go build ./...` (production — NO fuzzing symbols) AND
  `go build -tags fuzzing -o rune-fuzz ./cmd/rune`.
- Drain terminates: unit-test the driver on a fresh model's `Init()` — assert quiescence (no hang)
  and `m.store != nil` after `StoreReadyMsg` injection (proves the watch-loop/Init-drain fixes).
- Sequence ordering: feed a state that emits `tea.Sequence(a, b)`; assert both Msgs reach `Update`
  in order (proves the reflection-based unwrap).
- Phase 0: run `rune-fuzz` under ttyd; drive the `forceViolation` seed; assert a frozen frame, a
  `screenshot.png`, and a `violation.json`.
- Decoder: `bytes→events` is total (fuzz a million random byte strings, assert no panic/error);
  `events→JSONL→events` round-trips to identity.
- Cell fidelity: `FuzzCells()` output equals the cells `View()` renders (both call `renderCells()`).
- Engine: `go test -tags fuzzing -fuzz=FuzzSession ./pkg/ui/pages/workspace -run=^$ -fuzztime=...`;
  confirm a planted bug (e.g., temporarily break the EOL synthetic-cursor cell) is caught + minimized.
- Isolation: after an engine run, `~/.local/share/rune/rune.db` mtime is unchanged (in-memory store).
- Determinism: replay a `keys.jsonl` N times → identical Violation + identical frame.txt.
- Regression: every shrunk repro re-runs green in CI after its fix.

## Decision Log (resolved)
- **D1 Architecture**: rune owns invariant-checking + keystroke-driving in-process; Playwright =
  screenshot/artifact tool only (real xterm pixels rune's View() can't self-certify).
- **D2 Key delivery**: input via ttyd's real pty path, injected in CLUSTERS (amortized per-key
  cost). [superseded for bulk discovery by D8 — ttyd now per-finding + decode-smoke only.]
- **D3 Invariants (all 5, render-first)**: RENDER R1–R7 (screenshot-provable); SHADOW oracle;
  TAB BAR T1–T2 (no duplicate paths, active-index in range); DATA-LOSS (HARD STOP); MODEL cheap
  live re-checks. R2 respects reveal/render hiding.
- **D4 Violation flow**: freeze-on-first, fresh process per session; sentinel + artifact dir;
  Playwright screenshots the frozen frame.
- **D5 Cadence**: CheckInvariants after EVERY settled Msg (covers the tea.Batch async tail).
- **D6 Runtime**: synchronous deterministic driver (drain Cmds inline; collapse timers; logical
  clock). NO SLEEP; races out of scope. Enables reliable shrinking.
- **D7 Generation**: layered — LLM seeds + grammar gen + mutation ops + delta-debug shrinker;
  regression corpus. Event-script JSONL format (NEW).
- **D8 Topology**: Playwright per-finding only; in-process does discovery + shrink; one-time
  decode-smoke through ttyd.
- **D9 Engine**: hybrid — Go-native `go test -fuzz` (coverage-guided penetration + free min) +
  bytes→event decoder + LLM seed corpus + event-level shrinker for readable repros.
- **D10 Gating**: build tag `//go:build fuzzing` → `rune-fuzz`; production compiles none of it;
  shared logic in plain `internal/fuzz/*`.
- **D11 Artifacts**: per-finding dir with violation.json, keys.jsonl (+full+seed), frame.txt/
  .ansi, cells.json, db.sqlite, journal.jsonl, stack.txt, screenshot.png, meta.json (JSON/JSONL).
- **D12 Isolation**: `docstate.OpenInMemory(clock)` for Phase 0/1 (no disk, no path threading);
  `OpenAt(baseDir)` + injected clock for Phase 2 DATA-LOSS on real files; store injected via the
  exported `StoreReadyMsg`; shadow re-applies recorded AppliedEdits to an independent mirror.
- **D13 Scope**: thin end-to-end slice first (validate screenshot plumbing), then deepen.
- **D14 Inspection (gated)**: the reusable driver/invariant packages are EXTERNAL to package
  `workspace`, so they read model state through gated `//go:build fuzzing` accessors
  (`FuzzInspect()` + markdownedit accessors) rather than moving the checker into `workspace`.
- **D15 Cmd drain (reflection)**: detect slice-of-Cmd messages by underlying type `[]tea.Cmd`,
  not by name — the only way to drain the unexported `sequenceMsg`. Self-rescheduling/blocking
  Cmds (watch loop, image `tea.Tick`, timer sleeps) are gated to no-ops; production is untouched.
- **D16 Decoder format (G2)**: fuzz input is a **binary length-prefixed** stream (discriminant byte
  + per-kind payload), decoded by a TOTAL decoder so small byte mutations → small event mutations
  and no input is wasted. JSONL is the human/LLM repro/seed format ONLY; `events ↔ JSONL` converters
  bridge the two.
- **D17 Forced violation (G3)**: a `//go:build fuzzing` `forceViolation` event injects a synthetic
  `Violation` so Phase-0 freeze→sentinel→screenshot→artifact is validated with no dependence on a
  real bug; production never compiles the hook.
- **D18 Inspection refinements (B4/G4/G5/G6)**: `FuzzCells` reuses an extracted unexported
  `renderCells()` helper shared with `View()` (no stored cells, no drift); `DrainEdits()` is already
  public so `FuzzRecordedEdits` is dropped; docstate path and filetree workdir are redirected
  independently; the sentinel is emitted by the driver-owned output loop, not by `workspace.View()`.
