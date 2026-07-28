# TODO — Go reference implementation

Scope: the Go implementation under `golang/`. Paths below (`pkg/…`, `cmd/rune`, `internal/…`) are relative to this directory; entries written before the move to `golang/` spell them without the prefix.
The Rust workspace at the repo root keeps its own list in the repo-root `TODO.md`.

## FuzzWorkspaceTabOps: intermittent "fuzzing process hung or terminated unexpectedly: exit status 2" (pre-existing, environment/resource flake)

**Status:** open; confirmed pre-existing independently by all three statechart-refactor tracks (T, W, M) — NOT caused by any of them.

- **Symptom:** `make test-fuzz` (or `go test -tags fuzzing -fuzz='^FuzzWorkspaceTabOps$' ./pkg/ui/pages/workspace`) occasionally reports a worker crash with no invariant name or panic trace — just "fuzzing process hung or terminated unexpectedly: exit status 2" — and writes a "failing" corpus entry under `pkg/ui/pages/workspace/testdata/fuzz/FuzzWorkspaceTabOps/<hash>`.
- **Not reproducible standalone:** every entry captured this way (e.g. `666b114d7066f71e` from the rebased T-branch run, `7e4aa989fadca355` from a control run against unmodified `a7bbc62`) PASSES cleanly when re-run in isolation via `go test -run=FuzzWorkspaceTabOps/<hash>` — the input itself is not a real invariant violation, the parallel fuzz run just loses a worker (resource contention under N=14 workers, not a deterministic bug in the input).
- **Confirmed pre-existing:** reproduced against the untouched `a7bbc62` baseline in a throwaway `git worktree add <tmp> a7bbc62` — same signature, same standalone-pass behavior. T3's commit message independently documented the same flake during the original (pre-rebase) T-track run, with a different corpus hash (`3a0021192dd9736f`), also root-caused as environment/resource, not a Track T regression.
- **Action:** none required for the statechart refactor landing. The generated `testdata/fuzz/FuzzWorkspaceTabOps/*` "failing" entries are harmless (they pass when re-run) but are left untracked/uncommitted deliberately — committing a flake-only entry would make `make test-fuzz` look red for the wrong reason. If this becomes disruptive, the real fix is investigating why a fuzz worker process can die under this target specifically (fd/goroutine/sqlite-temp-file exhaustion under 14 parallel workers are the leading suspects, unconfirmed).
- **Reconfirmed during the T1–T6 rebase-onto-main landing:** hit again on a completely fresh corpus directory (no prior seed entries at all) at `make test-fuzz`'s default 1-minute budget, ~12s in, hash `06b04289206359b8` — same signature, same standalone pass. Three independent hashes now observed (`3a0021192dd9736f` at T3's original commit, `666b114d7066f71e`/`7e4aa989fadca355` from the T=20s gate on the rebased branch and the `a7bbc62` control respectively, `06b04289206359b8` from this full-budget rerun), always ~10-20s into the run regardless of which input is "blamed" — consistent with a timing/resource-window flake, not a specific bad input. The generated corpus files were deleted after standalone-verifying each one (not committed) to keep `make test-fuzz` reproducible for the next runner; this note plus the git history of this file is the retained evidence.
- **Track W independently bisected the same flake** (`git stash -u` back to clean `a7bbc62`, identical invocation, identical signature and hash `06b04289206359b8`) and observed the misattribution mechanism: under Go's parallel fuzzing, the "failing input" written is often just whatever was in flight when a worker died, not the actual trigger — the captured seed passes standalone on every commit. Track W's final full `make test-fuzz` run (14 targets, default budget) passed cleanly INCLUDING this target, confirming intermittency. Track M reproduced it on clean `a7bbc62` as well, with a CPU profile showing >85% of time in `runtime.kevent/madvise/pthread_cond_*` — no product hot path — pointing at a worker-liveness watchdog under CPU contention.
- **Next step if it becomes disruptive:** dedicated investigation with a single fuzz worker (`GOMAXPROCS=1` or explicit worker count) to isolate the real culprit input, per the project's "reproduce hangs with polled evidence" convention; leading suspects are fd/goroutine/sqlite-temp-file exhaustion under 14 parallel workers. Alternatives: a lower `-parallel` for this target in `make test-fuzz`, or checking whether an unusually heavy input among the ~417 committed corpus entries exercises a genuinely slow path (many synchronous SQLite commits) worth speeding up in its own right.

## Fuzz catch: RESIZE-INV — HasDirtyFile flips true→false on resize (pre-existing)

**Status:** open; investigation started 2026-07-12 and stopped by user before root cause landed.

- **Repro:** copy the failing input to `pkg/ui/pages/workspace/testdata/fuzz/FuzzSaveRace/50d39e00116f9609`, then
  `go test -run 'FuzzSaveRace/50d39e00116f9609' ./pkg/ui/pages/workspace/`
  → `session_fuzz_test.go:438: invariant RESIZE-INV: HasDirtyFile changed on resize: true → false`.
  The input file is NOT committed (it would make `make test` red — it was briefly committed at `229c0ba` and reverted at `fa7b014`). A copy lives outside the repo; regenerate by re-running the reverted commit's diff (`git show 229c0ba`) which contains the 2-line corpus file verbatim.
- **Bisection (done, trust it):** fails at `7bba191` (QA-rehaul merge, where the RESIZE-INV monitor was introduced) and at every later commit including `b0551da` — i.e. it predates ALL of the 2026-07 pkg/ui refactor stages (A1–A3, B1–B7). The pre-rehaul commit `7f08432` has no such fuzz subtest, so whether the underlying bug predates the rehaul is unknown.
- **ROOT CAUSE (investigation complete, verdict: genuine product flaw, not a monitor bug):** `syncDirty()` (workspace_edit.go:225-236) polls `store.IsDirty` from `finalize()` on EVERY message. The failing input is Paste → ⌘S → Resize: `store.Materialize` commits `saved_obs` inside the save Cmd's goroutine, so `store.IsDirty` flips false BEFORE `FileSavedMsg` is delivered; the next message of ANY kind (here a resize) re-polls and flips the tab's dirty display in the commit→ack gap. The resize is the messenger, not the mutator — real in production too, not just under the fuzz harness's deferred delivery.
- **Fix design (USER DECISION 2026-07-12 — eliminate the shadow state, don't retime the poll):** the root problem is a parallel source of truth: `Tab.Dirty` (and parts of `activeSave`) shadow the DB, and any ambient read (`syncDirty`'s per-message `store.IsDirty` poll) can observe the DB mid-transition and contradict the pending ack. An event-driven `refreshDirtyFromStore(...)` would only narrow the race. Required shape instead:
  1. **Displays update ONLY from operation results, never ambient reads.** Each store mutation carries the post-mutation dirty bit in its OWN return value — `AppendEdit` (synchronous, main loop) returns it; `Materialize`'s `MatResult`/`FileSavedMsg` carries it; `Load`'s `FileLoadedMsg` carries it (a recovered doc with journal head ahead of `saved_obs` opens dirty); undo/redo's `MoveUndoPos` result carries it; merge/discard resolutions land through those same paths. The UI applies the bit co-atomically at the ack. Delete `syncDirty()` and, ideally, remove the UI's ability to call `store.IsDirty` outside decision points — the capability, not just the call site.
  2. **`activeSave` sheds DB-duplicating fields**: it may only record "async op in flight, ack pending" (a message-ordering fact); anything the DB also knows must arrive in the result message instead of being cached UI-side across the gap.
  3. **Decision points unchanged** (§1.4.8): `vetSave`/`groundTruthDirty`/`enforceTabLimit` keep re-deriving fresh — safe because synchronous within one Update pass.
  Fail-safe direction still holds: a lost ack leaves the display dirty, never falsely clean. `syncDirty()` is currently the ONLY `Tab.Dirty = true` raiser — the operation-result plumbing must cover every raise transition it masked.
- **Sequencing:** implement AFTER the A4–A7 worker lands (it is actively editing workspace_edit.go/finalize — fixing now would collide).
- **When fixed:** re-add the corpus entry in the SAME commit as the fix (catches travel with the repo, but only alongside their fix).

## Layering: pkg/docstate/store.go ships test support in the production binary (pre-push review PP-2)

`store.go` imports `testing` + `internal/editortest` to host `NewTestStore`/`AutoClock` (QA-rehaul convention; footer_testing.go documents the same cross-package test-seam pattern without the import). Not a runtime bug — but the shipped binary links the testing package. Proper fix per CONSTITUTION §50 spirit: move `NewTestStore` behind `//go:build testing` (requires adding `-tags testing` to Makefile `test:` targets + CI + developer habit) or into a `docstatetest` subpackage (requires migrating ~12 test files' imports). Both are convention decisions — pick one deliberately. Related fragility notes from the same review: two_sessions_fuzz_test.go shares one non-thread-safe AutoClock across two stores (mutex or comment); editortest.Drain has no iteration cap (a steps-cap panic-guard would fail loudly instead of hanging on a future undisabled timer); invarianttest.CheckWorkspace silently skips L1/L2 (Frame never set on the unit path — fix the doc claim or plumb a frame).

## Statechart refactor — deferred edges (plan-statechart-refactor.md, deliberate scope cuts)

- **E3 — `DisplaySeq` reveal signal (statechart pressure point #8):** textedit could expose `DisplaySeq() uint64`, bumped whenever `syncDisplay` installs a display snapshot that differs from the previous one (content, reveal, wrap, or image-expansion). markdownedit's `reconcile` could then skip the whole `afterMutation` funnel when both `rev` and `DisplaySeq` are unchanged, and `hasUndiscoveredImages` could be deleted (≈ −12). Not needed for correctness — the funnel is change-gated internally — pure efficiency/clarity follow-up.
- **E4 — workspace divider `drag` outlives the mouse button:** the workspace's own divider-drag field persists until the next click. `tea.MouseReleaseMsg` exists in Bubble Tea v2.0.6 and reaches children via the workspace's default broadcast; the divider is its natural first consumer. (markdownedit deliberately has no release handler — after the refactor it keeps no drag state at all.)
- **Statechart pressure point #10 — placement double-tick:** the iTerm2 placement pipeline's "pending ⇔ one tick" invariant is only *mostly* exact (an overwrite edge emits a second tick that safely no-ops). A sequence number would make it exact. Harmless today.

## Fuzz catches: FuzzHumanSession tab-coherence invariants — EDITOR-TAB-COH and TAB-SET (both pre-existing)

**Status:** open; found 2026-07-16 while fuzzing the integrated statechart-refactor tree. BOTH bisected to PRE-EXISTING — each reproducer fails identically on `a7bbc62` (pre-refactor base), on T+M (`53b3d08`), and on W's tip (`6e02700`), so no refactor track introduced them. FuzzHumanSession surfaced two distinct violations in back-to-back 1-minute runs — its latent bug surface in tab management is evidently not yet drained; expect more catches until the underlying coherence flaw(s) are fixed.

- **Catch 1 — EDITOR-TAB-COH:** corpus entry `FuzzHumanSession/42335ab630528b99`, content:
  ```
  go test fuzz v1
  []byte("y0 0c")
  ```
  → `human_fuzz_test.go:117: invariant EDITOR-TAB-COH: EditorPath "/fuzz/notes/e.md" != Tabs[0].Path "/fuzz/a.md"` (deterministic, 0.09s). The editor ends bound to one doc while the active tab points at another.
- **Catch 2 — TAB-SET:** corpus entry `FuzzHumanSession/d9abe6755ff8bd82`, content:
  ```
  go test fuzz v1
  []byte("11A920\xaf{\xed\xf9C5'\xdf\xd8\xf6\xdb\x106\x1b\x84\xc3\xf3H&\xbb\x9f\x99G\x86ni\xc5.2")
  ```
  → `human_fuzz_test.go:117: invariant TAB-SET: expected exactly 1 active tab, got 0` (deterministic, 0.23s). A session ends with no active tab at all.
- **Repro:** write the entry file under `pkg/ui/pages/workspace/testdata/fuzz/FuzzHumanSession/` then `go test -tags fuzzing -run 'FuzzHumanSession/<hash>' ./pkg/ui/pages/workspace`.
- **Not committed while red** (same convention as RESIZE-INV above): re-add each corpus entry in the SAME commit as its fix. Copies of both inputs are quoted verbatim here.
- **Next step:** root-cause with the read-only fuzz investigator (`/rune-fuzzer` catch flow); suspect area is the tab-switch/load-settle/close path that updates `view` and `opentabs.SetActive` asymmetrically. Both invariants smell like one underlying coherence flaw around doc transitions.

## go workspace — TAB-SET orphan: a failed close→neighbour load strands the workspace with no active tab (recorded 2026-07-28, found by `make test-fuzz` during the Rust rename work; PRE-EXISTING, unrelated to it)

Deterministic repro (<1s), seed written by the fuzzer to
`pkg/ui/pages/workspace/testdata/fuzz/FuzzHumanSession/2ddbeeb17e97700d` = `[]byte("!020c")`:

    go test ./pkg/ui/pages/workspace -run='FuzzHumanSession/2ddbeeb17e97700d'
    human_fuzz_test.go:117: invariant TAB-SET: expected exactly 1 active tab, got 0

Minimal human repro, independent of the seed's Help-toggle framing: **open two files, delete
the neighbour's file externally, then close the active tab.**

**Root cause.** `executeClose` (`workspace_nav.go:248-265`) sets `m.view = untitledView(0)` — the
deliberate save-safe transitional identity — and issues an async load of the neighbour.
`finalize` (`workspace_edit.go:279-292`) covers that gap *only while* `m.pendingLoad.active`,
deriving the active tab from `pendingLoad` (the documented one-hop lead). When the load fails,
`handleFileLoadErrorMsg` (`workspace_update.go:275-299`) clears `m.pendingLoad` at `:284` but its
re-anchor branch is gated on `msg.Gen == 1 && len(m.initialFiles) > 0` — startup only — so it
returns **without restoring any document identity**. `finalize` then runs with
`m.view.Handle() == {0, ""}`, and `opentabs.SetActive` (`components/opentabs/opentabs.go:175-198`)
falls off the end at `:197`, storing an `activeHandle` that names no tab. So this is not a path
that skips `finalize`; it is finalize running with a handle matching no tab.

**Blast radius is larger than a missing highlight** — the stranded state is reachable and sticky:
- The editor still holds the **closed** document's text (anti-flash buffer never cleared) and
  renders it as if current once the pending-load gate lifts.
- `journalEditOK` (`workspace_journal.go:110-112`) returns false for `docID == 0`, so keystrokes
  typed there are **never journaled** (§1.4.3) and ⌘S is inert. Type-into-the-void.
- `requestCloseCurrent` returns unchanged for an untitled view (`workspace_nav.go:239-241`), so
  **^W cannot dismiss the orphan**.
- `EvictionCandidate` (`components/opentabs/eviction.go:48`) skips the *active* tab; with
  `activeHandle` matching nothing the orphaned tab loses that protection and becomes an eviction
  victim — a destructive §1.4.8 decision.
- `ActiveTabIdx` (= `nav.Cursor`) still reports 0 while no tab is active, so T2 and TAB-SET
  disagree about the same state.

`supersedeLoad` (`workspace_nav.go:130-134`, called from `:96` and `:369`) and
`handleFileLoadedMsg`'s gate lift (`workspace_io_handlers.go:36`) clear `pendingLoad` without
re-anchoring either — so a spot guard in the error handler alone is insufficient.

**Recommended fix (architectural — remove the illegal states, don't guard them).**
1. *Workspace:* make the close→neighbour transition an explicit `docView` kind carrying the
   target `TabHandle`, instead of the in-band sentinel `view == untitledView(0) &&
   pendingLoad.active` that three sites currently sniff (`workspace_edit.go:289`,
   `workspace_update.go:293`, and T4's commentary). `finalize` then derives the active tab from
   the transition target unconditionally, and the load-error handler cannot merely clear a bool —
   it must transition *out* (re-anchor to the failed tab with the buffer cleared, or close it and
   fall to the next neighbour / `CreateUntitled`). This also deletes the
   `gen==1 && len(initialFiles)>0` heuristic, fixes the stale-closed-document buffer and the
   unjournaled-keystrokes trap, and matches the direction of the HSM refactor (f64b829).
2. *opentabs:* replace the free-floating `activeHandle` with an active **index** into `m.tabs`
   (invariant `len(tabs) > 0 ⇒ 0 <= activeIdx < len(tabs)`, maintained by `OpenFile`/`Close`/
   `SetActive`, which already compute the needed indices — `Close` has `NeighborOf`). Then
   `SetActive` physically cannot record a handle naming no tab, and `ActiveTabIdx` and TAB-SET
   can no longer disagree. At minimum `SetActive`'s silent fall-through at `opentabs.go:197`
   must stop being a no-op-that-corrupts.

Fix 2 alone downgrades the failure to "a stale tab stays highlighted while the editor shows an
unnamed buffer" — better, but still the type-into-the-void state. Fix 1 is the root-cause fix;
2 is the structural backstop.

**Tests that must move with the fix:** `workspace_pendingload_test.go:108`
(`TestPendingLoad_FailedCloseNeighbourIsSaveSafe` — currently *pins the parked shape as intended*;
keep its save-safety assertion, drop "identity stays 0/\"\" forever"). `:177` (T6) and `:204` (T7)
pin the intentional one-hop lead and should stay green. `:542-564` documents that the
`gen==1 && len(initialFiles)>0` heuristic exists solely to tell startup apart from this shape.

**Corpus decision left open:** the seed file above is currently **untracked**. Committing it pins
a genuine regression seed but leaves `make test-fuzz` red until the fix lands; it is held out of
the rename commits so the branch's own gates reflect the rename work. Decide when scheduling the fix.

## go tui — a long CJK-wrapped row's trailing padding is literal TAB bytes, not spaces (recorded 2026-07-28, glyph-grid parity plan, defect-fix session — investigated, NOT fixed)

**Status:** investigated in depth; root cause traced to a third-party dependency's cursor-movement optimization, not to any of this repo's own `pkg/`/`cmd/` code. Per the task's own instruction ("fix it if the fix is clear and small; if it is not, write up what you found and leave it") — this is neither, so it stays open.

**Repro:** `scripts/parity/fixtures/cjk.md` contains "A long CJK line to check wrapping at a narrow viewport width when every glyph counts for two columns instead of one: 这一行文字足够长应该会换行。", which wraps to two visual rows at the parity harness's pinned 120×34 size. `PARITY_SCENARIO=01-open-file scripts/parity/capture.sh go 01-open-file cjk.md` then inspecting `.scratch/parity/out/go.txt` shows the SECOND (wrapped-continuation) row ending `...这一行文字足够长应该会换行。\t\t\t│` — three literal `0x09` bytes fill the row's remaining width instead of spaces, right before the pane's right border. Reproducible on demand; does not happen on any ASCII-only wrapped row in the same fixture.

**Exhaustively ruled out — not in this repo's own rendering code.** Grepped the whole `pkg/`/`cmd/` tree for any tab byte used as filler/padding (`'\t'`, `"\t"`, `\x09`, `text/tabwriter`): every hit is either a real user-typed tab (`pkg/ui/components/textedit/commands_edit_lines_indent.go`, `pkg/editor/display/wrap_map.go`'s/`snapshot.go`'s actual-tab width math) or unrelated (keybind parsing). `textedit/render.go`'s `RenderView` — the ONE place a row's remaining width gets filled — fills with `' '`/`"~"` literally and delegates the final pad to `lipgloss.NewStyle().Width(m.width).Render(content)`, which pads with spaces (confirmed by reading `charm.land/lipgloss/v2`'s `align.go`/`style.go` directly: `strings.Repeat(" ", shortAmount)`, and `maybeConvertTabs` only ever turns an EXISTING tab into spaces, never the reverse). Calling `textedit`/`markdownedit` directly (bypassing the real terminal) with the exact fixture text at every width 20–130 produced zero tab bytes in either focused or unfocused state — confirming the bug isn't reachable without a real pty/terminal in the loop.

**Root cause, in a dependency this repo vendors via a `replace` directive (`go.mod`): `github.com/charmbracelet/ultraviolet` is replaced with a fork, `github.com/aka-rider/ultraviolet`.** That fork's `terminal_renderer.go` (`relativeCursorMove`, ~line 1313; `moveCursor`, ~line 1466) implements a byte-cost-optimized cursor-movement scheme: when the diff engine decides a span of cells doesn't need repainting, moving the cursor forward across it can use literal Horizontal Tab bytes to jump 8-column tab stops instead of a longer ANSI cursor-forward escape sequence —

```go
// terminal_renderer.go:1362-1375 (github.com/aka-rider/ultraviolet)
if useTabs && s.tabs != nil {
    ...
    if tabs > 0 {
        cht := ansi.CursorHorizontalForwardTab(tabs)
        tab := strings.Repeat("\t", tabs)
        if false && s.caps.Contains(capCHT) && len(cht) < len(tab) {
            // dead code — the CHT-preferring branch is permanently disabled
            seq.WriteString(cht)
        } else {
            seq.WriteString(tab)
        }
```

`moveCursor` picks this candidate whenever it's the byte-cheapest way to reach the target column — routine for a long rightward jump toward a pane's right border. Hard-tab optimization is wired on by default for any `$TERM` other than `"linux"` (`bubbletea/v2/cursed_renderer.go`'s `setOptimizations`/`SetTabStops`, itself driven by the pty's own termios `TABDLY` flag, `termios_bsd.go`/`termios_unix.go` — no public `tea.ProgramOption` exists to disable it). tmux, receiving these raw `0x09` bytes, processes them as real horizontal-tab cursor movement and — per its own internal grid model (which keeps a literal tab marker in the cell it was invoked from, so a later resize can re-expand/reflow it correctly) — is what `capture-pane -p` then faithfully reproduces as literal tab characters in its plain-text dump.

**Why this specific row (CJK-heavy, second wrap segment) and not an ASCII one:** the hard-tab cursor-skip only fires when the diff engine's belief about "where this row's real content ends, and where an unchanged/reusable tail begins" is off from the true rendered width — the display-width source of truth for THIS row is `pkg/editor/display/wrap_map.go`'s `ControlAwareWidth`/`runeWidthWithTab` (feeding the greedy wrap loop) versus `pkg/ui/components/textedit/cell.go`'s own per-cell render width; any drift between the two shifts the renderer's idea of the row's changed/unchanged boundary — and a CJK-heavy row's column parity (every glyph width 2, not 1) makes a long tab-stop-crossing cursor skip land differently than it would over the same BYTE count of ASCII, which is presumably why this is the row that trips it and an ASCII-only wrapped row does not. This mismatch was NOT pinned down further (would need instrumenting the vendored fork's diff engine directly, a materially bigger undertaking than this task's scope).

**Two possible fix directions for a future task** (neither attempted here, per "do not force a speculative fix into the Go renderer"): (a) patch the vendored `aka-rider/ultraviolet` fork to stop unconditionally emitting hard tabs for a cursor skip whose target cells might not actually match the renderer's assumption, or gate/disable hard-tab optimization entirely for this application; (b) find and fix whatever specific CJK-width mismatch between `wrap_map.go` and `cell.go`/the renderer's own diffing causes the "unchanged tail" boundary to be computed wrong in the first place — the latter is the fix within this repo's own domain, the former is what actually stops the tab bytes from reaching the terminal.

`scripts/parity/fixtures/cjk.md` stays excluded from `parity-grid` (`scripts/parity/grid.sh`'s `excluded_reason`, `scripts/parity/README.md`'s "Known divergences") for this reason, unchanged from before this investigation — now with a concrete, verified root cause instead of "not yet identified."
