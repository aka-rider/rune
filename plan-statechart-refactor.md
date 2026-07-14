# Rune Refactoring Plan — Statechart-Guided Simplification

## Context

`STATECHART-improved.md` is a verified hierarchical state-machine model of rune's UI layer. It shows the code implements a small number of machines (workspace key-owner chain + guards, textedit modes + chord resolver + search, markdownedit reconcile + placement + mouse, image lifecycle + animation) — but expresses them as scattered flags, duplicated dispatch epilogues, per-path after-hooks, six parallel intent structs, and dead states. This plan refactors each machine around its data ownership, deleting the repetition. **Net delta target ≈ −950 lines** across production + tests (tests grow only in the image region, where state-guard coverage is new).

Ground rules honored throughout:

- Net code delta negative; readability and extensibility not compromised.
- Data ownership first; explicit machines only where a machine actually exists. Three verdicts follow the statechart's own rules: the **image lifecycle** gets an explicit `transition/enterState(prev)` machine (it has real states, real entry actions, and two pressure-point bugs); the **guard lifecycle** keeps its enum + closed transition-function set (those functions *are* the EnterState hooks; an interface-based HSM would add ~40 lines to replace ~30); everything else (focus, modes, key dispatch, reveal) stays enum/derived — per statechart rules 1–3.
- Constitution binds: no panics, journaled installs, D3 (read-only drops installs silently — drained-empty is the only signal), D13 (`rev` is the only content-changed signal).
- Each step is a separate commit gated by `make build` → `make test` → `make test-fuzz`, so a fuzz catch bisects to one mechanical change.

**Verified foundations** (checked in-tree, not just by the statechart):
- `keymap.parseChord` always returns a one-element chord list (`keymap.go:288`) and `CommandBindings` filters `"enter"`/`"esc"` (`keymap.go:296-298`); `CommandBindings` is the only production binding producer (app.go:63, fuzz driver). ⇒ the resolver's Pending state is **unreachable in production**, not merely indistinct — it can be deleted, not just reset.
- `MarkFailed`/`Failed` unreachable; `ErrorMsg` routed but unhandled; `dragAnchor` write-only; `dragging` never read for behavior; `InCodeFence` never set and no binding uses the atom.
- Bubble Tea v2.0.6 delivers `tea.MouseReleaseMsg`, but markdownedit keeps no drag state after the dead-code deletion — no handler needed (documented in a comment; the workspace divider drag is a natural future consumer).

## Step 0 — Land the in-flight work first

The working tree holds a coherent uncommitted change (CursorID on `buffer.Edit`, `lastEdits` provenance for the SEL-EDIT fuzz invariant, commands_clipboard.go already deduped −71 onto the shared `editInfoItem` helpers). Commit it as-is (its own `make test-fuzz` gate). Every later step builds on those helpers.

## Track order

**T (textedit/resolver, ≈ −620) → M (markdownedit/image, ≈ −53 prod, tests +100) → W (workspace, ≈ −280).** T first because M's funnel relies on T3's rev semantics being settled; W last because it has the largest test churn (154 `guard.*` refs) and benefits from a stable editor underneath. Tracks are independent per machine; only the flagged edges cross.

---

## Track T — textedit + chord resolver + search (§2, §2.1, §2.2)

### T1. Stateless resolver; footer becomes the single chord authority (≈ −195)

`pkg/editor/keybind/resolver.go`:
- Delete `ResolveTimeout`, `Reset`, `InChordMode`, `PendingDisplay`, `formatChord`, fields `pending` + `timeoutCommand`; delete `ResultMoreChordsNeeded` from the enum; delete `ResolverContext.InCodeFence` and the `codeFence` atom in `parser.go`.
- `Resolve` becomes a pure read-only method (linear scan → `Found`/`NoMatch`); the per-keypress `m.resolver = newResolver` reassignment in `update.go` disappears.
- `NewResolver` errors on `len(b.Chords) > 1` ("chord sequences are not supported") — config stays honest instead of silently never matching. Doc comment on `Resolver`: config-only; reintroduction recipe = component-owned `pendingChords []Chord` on textedit.Model, cleared on blur.
- `resolver_test.go`: delete pending/timeout tests (~110 lines); add one multi-chord-errors construction test.
- Footer: dedupe the two identical quit-chord branches (`footer.go:165-179`) into `confirmChord(k string)`; comment on `pendingKey`: now the app's only cross-key chord state (its timeout is live UX, unlike the dead resolver timeout).

Resolves pressure points #2, #3, #9. Fuzz chord observability becomes complete (footer already snapshotted, builders.go:89).
Gate: build, test, grep-zero for deleted symbols, `make test-fuzz`.

### T2. `applyResult` epilogue + fast-path simplification (≈ −34)

```go
func (m Model) applyResult(res command.Result) Model {
    m = m.applyOperation(res)
    m = m.syncDisplay()
    if res.Operation.Kind != command.OperationScroll { m = m.ScrollToCursor() }
    return m
}
```
Replaces the 5 trio sites: update.go:74-76 (Enter), :89-91 (Escape), :143-150 (Found — conditional subsumed), :175-177 (NoMatch insert), commands_clipboard.go:230-232 (paste). Enter/Escape stay hardcoded (filtered from CommandBindings), but both fast paths now end in an **unconditional return** — with a stateless resolver the fall-through is provably inert. Delete the dead `keys` field on Model; add `basicCtx()` micro-helper.
Gate: build, test, fuzz (M1 invariant).

### T3. `commitEdits` mutation chokepoint (≈ −12)

```go
// The single buf-swap / pendingEdits / lastEdits / rev++ site (D13).
func (m Model) commitEdits(newBuf buffer.Buffer, applied []buffer.AppliedEdit,
    journal bool, provenance []buffer.Edit) Model
```
Callers: `applyOperation` (journal=true, provenance=op.Edits), `ReplaceRange` (true, nil), `ApplyInverse`/`Reapply` (false — undo/redo must not re-journal). `SetContent` stays explicit (it *resets* pendingEdits — document-identity swap). Sync/scroll stay with callers (cursor decisions precede sync).
**Flagged tightening**: a failed/empty edit apply no longer bumps `rev` (strictly more correct per D13; markdownedit's funnel then takes the cursor-move branch, which is right). Fallback if fuzz disagrees: one-line restore.
Gate: build, test (reapply/replace_range tests), fuzz (B2, SEL-EDIT), run the committed crasher corpus.

### T4. Two command drivers (≈ −250)

In `commands_edit_lines.go` (the helper family home):
```go
func noneResult() command.Result
// selection-exact commands — the SEL-EDIT CursorID-tagging boundary
func perCursorSelectionEdits(ctx command.CommandContext,
    textFor func(i int, c cursor.Cursor) string,
    bare func(c cursor.Cursor) (start, end int, ok bool)) command.Result
// line-oriented commands — never CursorID-tagged
func perLineEdits(ctx command.CommandContext, dedupe bool,
    build func(line int, c cursor.Cursor) (buffer.Edit, bool)) command.Result
```
Collapses: insert-char, newline, delete-left/right, delete-word-left/right, paste loop → `perCursorSelectionEdits`; delete-line (**merged into** delete-line-multi, deleting the near-verbatim dupe), clone-up/down, indent, dedent → `perLineEdits`; multicursorAddAbove/Below → one `execMulticursorAdd(ctx, dir)` + wrappers. Deliberately not collapsed: moveLine, multicursorEscape, selectAll, nav family, `buildDeleteEdits`. Delete the fixed-insertLen helper variants (Var versions subsume). Update the SEL-EDIT tagging-boundary comments (fuzz driver + `infosToEdits`).
Gate: build, test (newline_sel_test), fuzz with SEL-EDIT as sentinel.

### T5. Registration tables (≈ −130)

`cmdSpec{name, when, exec, category, title}` + `registerAll(b, specs)`; each family exposes a specs slice; `RegisterCommands` concatenates once. Converts the 7 hand-unrolled Register blocks; commands_nav_gen.go drops its local entry type for the shared one. Gate: build, test — app.go's startup binding↔command verification loop is the real check.

### T6. `ClearSearch` delegates to `SetSearchQuery("", m.searchCaseInsensitive)` (−3)

searchRev symmetry by construction (pressure point #11). Gate: search_state_test, fuzz.

---

## Track M — markdownedit + image subsystem (§3, §4)

### M1. Dead code (≈ −20)

Delete `image.MarkFailed`, `image.Path()`, `mouseState.dragAnchor`, `mouseState.dragging` (+ comment on `mouseState` explaining why no release handler: no drag state left to reset; motion events carry the held button, so "drag" is per-event derivable). Gate: build, test.

### M2. Image lifecycle as an explicit machine (≈ +45 prod, +40 test)

The one step that costs lines — it buys pressure points #1 and #7 by making illegal transitions unrepresentable.

- **Spawn generation** (prerequisite: state guards alone would freeze stale pixels on mtime respawn): `image.Model.gen uint64` from a package atomic counter; `UpdateMsg`/`ErrorMsg` gain `Gen`; envelope guard drops `Path != m.path || Gen != m.gen`.
- **Transition chokepoint**:
```go
// transition is the ONLY writer of m.state; enterState receives prev because
// the two edges into PendingTransmit differ (initial transmit vs Resize retransmit).
func (m Model) transition(next State) (Model, tea.Cmd)
func (m Model) enterState(prev State) (Model, tea.Cmd)
```
  Entry actions: `PendingTransmit` from `PendingDecode` dispatches TransmitCmd/EncodeITerm2Cmd (unless animated → await SetFrameIDs); `Live` emits ReadyMsg (arming moves to M4's chokepoint); `Failed` stops ticking.
- **State × message guards** in `handleInner` (illegal = silent drop; stale-async by construction once Gen matches). Two deliberate accepts: `Live+encodedMsg` stores refreshed slices (iTerm2 retransmit), `Live+transmittedMsg` is an idempotent ack.
- Rebuild `Failed` properly: `err` field + `Err()`; `ErrorMsg` → `transition(Failed)`.
- Delete the `transmittedMsg` self-arm (image.go:135-137); `Resize` writes back `maxCols/maxRows` (fixes the latent config/runtime split).
- Tests: stale-gen drop, wrong-state drop, ErrorMsg→Failed, Failed-drops-everything.
Gate: build, test.

### M3. Error surfacing + retry rule (≈ +8)

In `updateImages`, on the ¬Failed→Failed edge emit the error to the workspace. Rename `ImageSaveErrorMsg` → `ImageErrorMsg` (edge E5 — one case label at workspace_update.go:185; the message now carries both paste-save and decode/transmit failures, so the old name would be a misnomer) with an updated doc comment. Retry rule in discovery: skip `PendingDecode` and same-mtime instances only — `Failed` is sticky per (path, mtime), an mtime change respawns and retries. Failed renders as ordinary markdown (Height()==1, placeholder gated on IsLive()) — no view change.
Gate: build, test; manual: corrupt image → footer error once, retry after `touch`.

### M4. `mapImages` + one-pass visibility + re-arm chokepoint (≈ −27)

```go
func (m Model) mapImages(fn func(path string, img image.Model) (image.Model, tea.Cmd)) (Model, tea.Cmd)
func (m Model) visibleRowsByPath() map[string]int   // one pass over the viewport window
// THE single animation re-arm chokepoint: project visibility, then re-arm.
func (m Model) syncImageViewState() (Model, tea.Cmd) {
    vis := m.visibleRowsByPath()
    return m.mapImages(func(p string, img image.Model) (image.Model, tea.Cmd) {
        img = img.SetVisibleRows(vis[p])
        return img.ArmTick()
    })
}
```
`mapImages` rewrites resizeImages/detectImageCollapse-loop/retransmitImagesCmd; `visibleRowsByPath` replaces the per-image `visibleRowsFor` scan and the duplicated geometry test. `syncImageViewState` is called from the M5 funnel tail (⇒ **keyboard scrolling re-arms** — fixes a real bug), from the wheel handler (replacing `armImageTicks`, which today arms with stale visibleRows — a second real bug), and from the layout-refresh path. `updateImages` drops its inline SetVisibleRows + ¬Live→Live arm block.
Gate: build, test; manual: GIF below the fold, keyboard-scroll into view → animates.

### M5. `afterMutation` — the single funnel (≈ −45)

Replaces `afterContentChange` + `afterCursorMove` and both shadow families; makes `reconcile`'s "single funnel" doc comment true:
```go
func (m Model) afterMutation(contentChanged bool) (Model, tea.Cmd) {
    // 1. syncImageSet (M6) when contentChanged || hasUndiscoveredImages()
    // 2. re-publish dims only when changed (new publishedDims map[string]display.ImageDims
    //    field gates SetImageDims — it runs a full display sync, must not run per keypress)
    //    + ScrollToCursor when dims changed
    // 3. detectImageCollapse → tea.ClearScreen on a true collapse edge
    // 4. syncImageViewState (M4)
}
```
Callers: `reconcile(prevRev)` → `afterMutation(rev != prevRev)` (the fork becomes a boolean argument); delete the text-paste special branch (falls through delegation — rev bumps); ApplyInverse/Reapply/ReplaceRange → `afterMutation(true)` via a shared `applyChecked(op)` error-guard helper; `insertTextAtCursors` → `afterMutation(true)`; `SetContent` body becomes SetContent + `afterMutation(true)` — **signature → `(Model, tea.Cmd)` (edge E1 — five workspace call sites, three chained through `.SetReadOnly`; see the edge-contract table for the per-site list and the uniform unchain-and-append rule)**; `SetRect` keeps its changed-guard, then resize + `afterMutation(false)` — **signature → `(Model, tea.Cmd)` (edge E2)**, folding `RefreshImagesAfterLayoutChange` in, workspace deletes the call at workspace_edit.go:270. Delete `DiscoverImages`.
Gate: build, test, **fuzz** (this step moves the most; undo/SetContent/journal paths route through the funnel).

### M6. `syncImageSet` — discovery + despawn (≈ +12)

Rename `discoverNewImages`; one snapshot pass building `standalone` (spawn source, unchanged) and `present` (any image-role span — **not** the standalone set: `isStandaloneImageLine` rejects Revealed spans, and presence must be reveal-stable or a caret pass would despawn live images). Despawn absent paths: `delete(m.images, path)`, free allocator IDs (`FreeAllForPath`, replacing the unused `FreeID`), Kitty `DeleteCmd` batch to free terminal pixel memory; iTerm2 needs nothing. Dedupe the New+store+Init spawn triple. Stale results for despawned paths drop at the map lookup + gen guard.
Gate: build, test; test: delete embed line → despawned; undo → respawned.

### M7. Mouse tolerance + mechanical dedups (≈ −18)

Multi-click: pixel-exact match → Chebyshev ≤ 1 cell, same 500 ms window. `blankCells(n)` helper (view.go dupe ×2, −7); `readStill(...)` prologue helper in image/commands.go (dupe ×3, −14); fix comment drift #12; rewrite `reconcile`'s doc comment.
Gate: build, test, fuzz (final track gate).

---

## Track W — workspace (§1)

Data-ownership move first: the 8 separately-validated guard payloads collapse into **two slots**, justified by verified coexistence rules (at most one prompt at a time, its payload consumed in the dispatch that clears it; close/evict/quit never coexist with each other but DO coexist with a prompt — R1, pinned by `TestConflictDuringCloseSave_CoexistsThenAbandonsClose`). `activeSave` does **not** merge with `guard.phase` (it exists for guard-less ⌘S saves and is absent for guarded background saves — a product machine would be mostly unreachable states). Guard mutator set shrinks to exactly five functions; the one sanctioned out-of-band mirror (`workspace_update_keys.go:46` phase=guardIdle on synchronous footer resolution) is untouchable — GUARD-PHASE-SYNC depends on it.

### W1. Continuation slot (≈ −50)

```go
type contKind int  // contNone, contClose, contEvict, contQuit
type continuation struct {
    kind contKind; requestID string
    victim opentabs.TabHandle; pendingOpenPath string  // evict
    saveLeft int                                        // quit
}
func (c continuation) owns(k contKind, requestID string) bool
```
Deletes closeIntent/evictIntent/quitIntent + isCloseSaveAck/isEvictSaveAck (~25 reference rewrites); `abandonDirtyContinuation` body → one assignment; `raiseDirtyGuard(kind, c)` absorbs the 3× abandon-set-raise prologue; fuzz snapshot switches to `cont.kind`.
Gate: build, test (mechanical churn), fuzz (GUARD-PHASE-SYNC, GUARD-STATE-COH, R1 test green in spirit).

### W2. Prompt slot + dispatcher-owned capture-and-clear (≈ −55)

```go
// Valid iff guard.kind ∈ {trash, conflict, deleted, raced}; kind is the discriminant.
type promptPayload struct {
    docID int64; path string
    freshObs docstate.ObsID       // conflict
    saved, fresh docstate.Observation  // raced
}
```
Deletes conflictIntent/deletedIntent/racedIntent/trashPath; `racedQueue` → `map[int64]promptPayload`. `handleDataLossGuardResponse` does `p := m.guard.prompt; m = m.clearGuardPrompt()` once per kind case (+ hoisted R1 abandon for conflict/deleted only; guardDegraded deliberately does NOT abandon — its [Y] resumes the close-save); handlers become pure consumers `handleX(p promptPayload)`; six `if !active` prologues delete. Accepted delta: a cross-kind prompt overwrite drops stale garbage eagerly (input paths can't do it; async paths already overwrote kind). Fallback if fuzz finds a counterexample: keep `raced` separate (only payload with a re-queue path).
Gate: build, test (handler signatures change), fuzz + legacy-corpora mapping.

### W3. `issueSave` chokepoint + `failContinuation` (≈ −60)

```go
type saveReq struct {
    prefix string; docID int64; path, content string
    expect docstate.ObsID; bindNew bool
    track bool  // arm m.activeSave — every foreground save
}
// The ONLY materializeStoreCmd wrapper: requestID stamping, co-atomic seq
// capture (§1.4.2/§1.4.8), and activeSave arming can never diverge per-site.
func (m Model) issueSave(r saveReq) (Model, string, tea.Cmd)
```
Collapses 7 sites: save / force-save / force-save-deleted / restore-theirs / bind / evict / quitsave (prefixes verbatim — `quitsave-` prefix match preserved); unified requestID `prefix-docID-nano`.
```go
// Shared refusal exit for every vet failure on a confirmed continuation save.
func (m Model) failContinuation(text string) (Model, tea.Cmd)
```
Collapses evictSave's 5 + quit's 3 refusal blocks (texts verbatim). startSave's ladder stays (must not abandon); the 3 vetSave ladders deliberately NOT unified (verdicts map to different actions per site).
Gate: build, test (saveident/saverace/vetsave/evict; requestID-shape assertions may need updates), fuzz (GUARD-STATE-COH is the point).

### W4. `installJournaled` chokepoint (≈ −70)

```go
// The single journaled buffer-install transition: install closure →
// bumpEpoch (if bump) → DrainEdits → D3 empty-drain refusal (single-sourced
// error) → journalEditOK (rolls back on failure) → resolveAdoptAt.
func (m Model) installJournaled(docID int64, verb string, sync docstate.SyncState,
    bump bool, install func(Model) (Model, tea.Cmd, error), cmds *[]tea.Cmd) (Model, bool)
```
Collapses the 4 copy-pasted teardowns: applyDiscardConflict, applyMergeConflict (closure wraps `mergemode.Enter` — it legitimately mutates both merge and editor), installDiskAhead (bump=false — load path already bumped; its preceding `SetContent(ours)` returns a cmd after E1 — append it, and keep `prevCursors` captured *after* SetContent), handleDataLossRestoreTheirs (re-arm `raiseRacedGuard(p)` on !ok unifies its 3 failure re-arms; sync zero-valued so adopt no-ops — restore-theirs Materializes instead). Accepted delta: restore-theirs's error goes async via `errorCmd` like the other three.
**Reviewer checklist for this step** (critic-flagged as load-bearing, verify line-by-line against the four bodies before merging): mergemode.Reset moved after the install is order-independent; prevCursors post-SetContent in installDiskAhead; restore-theirs zero-value sync makes resolveAdoptAt a no-op.
Gate: build, test (d3_merge_readonly is load-bearing), fuzz (Adoption Contract, DL properties).

### W5. Key chain: title-gate hoist + `stopDictation` (≈ −30)

**Keep the linear chain** — it IS the priority spec; a routing table is line-neutral at best and destroys the at-a-glance ordering. Hoist the title gate: `finalizesTitle(msg)` predicate + one gate between undo/redo and the global switch deletes the 9 identical prologues (⌘N merge-refusal hoisted before it, order preserved; Help's refusal stays inside toggleHelp). Add `stopDictation()` (2-line core: dict.Disable + footer.SetDictating(false)) called by `disableDictationForTransition` and the ~5 raw copies — dict/footer drift becomes structurally impossible. Update the workspace_nav.go:298-301 comment.
Gate: build, test (merge_modal, D11), fuzz — the fuzzer is the primary net for key-routing regressions.

### W6. `surfaceRaced(p, cmds)` raise-vs-queue helper (≈ −15, optional)

Collapses the 4 raise-vs-queue branches in workspace_io_save.go/raced.go. Do only if W1–W5 landed clean.

**Rejected in this track (evaluated, below the pay line)**: doc-install helper (per-site quirks make it net ≈ 0), finalize/broadcast-tail dedup (two different phases), `m.err = nil` sweep, sync→async footer-error conversion, generic HSM framework, routing table, activeSave+phase merge.

---

## Cross-machine edge contract

| Edge | Decision |
|---|---|
| `rev` semantics | T3 tightens: no bump on failed/empty apply. M's funnel takes `contentChanged=false` then — correct. W unaffected. |
| Editor install semantics | Frozen: read-only drops installs silently ⇒ drained-empty is the only refusal signal (D3); every mutation surfaces exactly once through DrainEdits. `installJournaled` (W4) is the single adaptation point if this ever changes. |
| E1 `SetContent` → `(Model, tea.Cmd)` | Accepted. **Five** non-test callers of `m.editor.SetContent` (critic-verified), uniform rule at every one: unchain, append the returned cmd (`tea.Batch` drops nils) — no site drops it. (1) workspace_io_handlers.go:130 — append cmd, delete the separate `DiscoverImages()` call at :134. (2) workspace_merge_fresh.go:307 (`installDiskAhead`) — `m.editor, dcmd = m.editor.SetContent(ours)`, append to cmds; this cmd IS the image discovery for the adopted content, dropping it would leave embeds unspawned until the next mutation. (3) workspace_nav.go:360 (`CreateUntitled`) — unchain from `.SetReadOnly(false)`. (4) workspace_view_switch.go:15 (`showHelp`) and (5) :43 — both chained through `.SetReadOnly(...)`, unchain likewise. |
| E2 `SetRect` → `(Model, tea.Cmd)` | Accepted. Folds `RefreshImagesAfterLayoutChange`; workspace deletes the call at workspace_edit.go:270 (`finalizeLayoutChange`). Lands in M5 with the W-side one-liners. |
| E5 `ImageSaveErrorMsg` → `ImageErrorMsg` | Accepted (one case label, workspace_update.go:185) — the message now carries decode/transmit failures too; keeping the old name would be a misnomer. |
| Keymap | Keeps filtering `"enter"`/`"esc"`; no one adds enter/esc command bindings (T2's inert-fall-through proof depends on it). |
| Chord authority | Footer `pendingKey` is the app's only cross-key chord state after T1. |

**Deferred — record in `TODO.md` at repo root** (per the no-silent-skip rule):
- E3: textedit `DisplaySeq() uint64` (a counter bumped when syncDisplay installs a differing snapshot) would let `reconcile` skip the funnel entirely on no-op updates and delete `hasUndiscoveredImages` (−12 more). Not needed now.
- E4: workspace divider `drag` field persists past button release; `tea.MouseReleaseMsg` (available in BT v2.0.6, reaches children via the broadcast) is its natural fix.
- Statechart pressure point #10 (placement double-tick sequence number) — harmless, minor.

## Final documentation step

Update `STATECHART-improved.md` to match the refactored reality: §2.1 becomes "stateless matcher — no machine"; §3's reconcile fork becomes the `afterMutation(contentChanged)` funnel; §4 gains the Failed state and gen guards; pressure points 1–7, 9, 11, 12 marked resolved (8 and 10 deferred per TODO.md). Delete the outdated `STATECHART.md` or mark it superseded (ask via commit message; it's untracked — leaving it is the safe default).

## Critic review (done)

An independent critic pass verified the plan against the tree: resolver deadness (incl. keymap.go:253 having its own local `formatChord`, so deleting the resolver's is safe; the one production `ResultMoreChordsNeeded` case at update.go:151 is inside T1's footprint), Var-subsumes-fixed cursor math for every line command, the delete-line/delete-line-multi verbatim-dupe merge, image message construction confined to the image package (Gen stamping uniform), E2's single SetRect caller, all eight guard payloads, requestID matching being equality/prefix-only (no code parses docID out of one), and fuzz invariant IDs M1/B2/SEL-EDIT/GUARD-* existing verbatim. One blocker was found and fixed above (E1 had 5 call sites, not 1). Accepted risks carried with fallbacks: T3 rev tightening (one-line restore if B2 trips), W2 single prompt slot (keep `raced` separate if fuzz finds a same-doc cross-kind counterexample), W4 ordering claims (reviewer checklist embedded in the step).

## Verification

Per step: `make build` → `make test` → `make test-fuzz` (mandatory on T1–T4, M5, M7, and every W step), one commit per step. Watched fuzz invariants: GUARD-PHASE-SYNC (workspace), GUARD-STATE-COH (save-ack ownership), SEL-EDIT (CursorID tagging boundary), B2 (rev/version), M1 (≥1 cursor), DL data-loss properties. Grep gates after deletion steps (zero references to `ResolveTimeout|InChordMode|PendingDisplay|timeoutCommand|InCodeFence|ResultMoreChordsNeeded|MarkFailed|dragAnchor`).

End-to-end (`make run`, kitty + iTerm2): load a doc with png + gif; keyboard-scroll the gif out of and back into view (must animate — new behavior); corrupt an image file (footer error once, embed line renders as text, retry after `touch`); paste an image; undo/redo across an embed; delete an embed line (instance despawned, no stale pixels); merge-conflict flow end-to-end (guard prompt → merge → resolve → save); quit with multiple dirty tabs.

Expected totals: T ≈ −620, W ≈ −280, M ≈ −53 production (+~100 tests) ⇒ **≈ −950 net**, dominated by real repetition removal, with the only additions being the image lifecycle machine (the one place a machine was missing) and its tests.
