# Plan: Split Editor Monolith into textedit + markdownedit

## Goal

Break `pkg/ui/components/editor/` (~6200 non-test LoC across 31 files) into two separate Bubble Tea components:

- **`pkg/ui/components/textedit/`** — base text-editing component (buffer, cursors, viewport, commands, generic cell rendering, find overlay, dictation)
- **`pkg/ui/components/markdownedit/`** — rich markdown extension (MarkdownSync, images, links, markdown-specific cell styling, code highlighting)

The existing `pkg/editor/*` domain engine (buffer, cursor, display, history, keybind, coords) stays unchanged.

## Architecture Decisions (Grill-Me Round)

These six decisions are authoritative and override any older inline text below that contradicts them.

**D1 — markdownedit composes textedit via an explicit `Update` wrapper, NOT via method-promotion override.**
Go method promotion is *static dispatch, not virtual dispatch*. If `markdownedit.Model` embeds `textedit.Model` and textedit's own `Update → dispatchOperation → syncDisplay` chain runs, every `m.syncDisplay()` inside it resolves at compile time to **textedit's** `syncDisplay` — a `markdownedit.syncDisplay` is never called. Relying on promotion for any method in the edit/sync/render cycle would silently drop image expansion and markdown styling.
Therefore: markdownedit **owns its own `Update`** that explicitly delegates to `textedit.Update` and then re-runs markdown post-processing. Promotion is used **only** for genuinely generic leaf queries (`Content`, `CursorOffsets`, `Selections`, `SetFocused`, `SetSize`, `Height`, `Focused`, `WantsModalInput`). Embedding remains (for field/query reuse), but no edit/sync/render behavior depends on promotion.

**D2 — Seam: textedit = provisional, markdownedit = final within the same pass.**
textedit runs edit + generic sync + a *provisional* `scrollToCursor` against the image-free snapshot. markdownedit then runs a single `afterContentChange()` tail on the image-expanded snapshot:
`expandImageRows → ScrollToCursor (absolute, re-run) → re-clamp (no delta) → discoverNewImages → detectImageCollapse → armImageTicks`.
`scrollToCursor` is absolute so re-running it is correct/idempotent. The `OperationScroll` branch is **not** absolute — markdownedit's tail must **re-clamp only** (recompute `maxTop` against expanded `TotalRows`), never re-apply `ScrollDY`. No visible flicker: `View()` runs only after `Update()` returns, so markdownedit corrects the provisional scroll before any render.
textedit exports the primitives the tail needs: `ScrollToCursor() Model`, `Snapshot()`, `SetSnapshot()`, `ContentHeight() int`.

**D3 — Mouse messages break the "always delegate then post-process" rule; markdownedit intercepts them.**
markdownedit's `Update` has an explicit `switch` with three special arms (`tea.MouseClickMsg`, `tea.MouseMotionMsg`, `tea.MouseWheelMsg`) handled by markdownedit's own handlers, plus a `default` arm that does delegate-then-tail. markdownedit's mouse handlers call textedit's **exported low-level helpers** — never `textedit.Update` (which would re-enter the switch and move the cursor on a link click). textedit therefore exports a wide low-level mouse API: `MousePositionCursor`, `MouseExtendSelection`, `MouseSelectWord`, `MouseSelectLine`, `MouseAddCursor`, `DisplayToBuffer`, plus accessors `SyntaxSnap()`, `WrapSnap()`, `Viewport()`. textedit **keeps its own `MouseClickMsg` arm** for standalone (title — N/A now, chat-prompt) instances that have no links.

**D4 — The command registry is a single shared union, injected everywhere (editor behavior is uniform across the app).**
There is one global registry + one shared resolver; `app.go` verifies at startup that every keybinding's command name exists in the registry. So per-component registries are impossible. `textedit.RegisterCommands` registers edit/nav/multi/clipboard/history **+ find stubs + mouse no-ops** (the stubs exist solely to satisfy startup verification; the overlay/mouse are driven by direct methods). `markdownedit.RegisterCommands` registers image/link/file. `app.go` builds `Build()` from both and injects the same `Registry` into every instance.
- `ResolverContext.ReadOnly` MUST be set from `m.readOnly` (monolith hardcodes `false` at `editor.go:398`).
- Markdown-only commands resolving inside a plain-textedit instance are tolerated as **silent no-ops** (its `dispatchOperation` ignores unknown operation kinds).
- **Save is a workspace-global** key: handled by the workspace acting on the main editor, before forwarding to any focused child.

**D5 — Scroll policy belongs to the caller, not to `SetContent`. Rewrite §5.7 for the real `ViewportState`.**
The editor uses a custom `ViewportState{TopRow, ScrollCol}` — **not** bubbles `viewport.Model`. The §5.7 snippet (`AtBottom()/YOffset/GotoBottom/SetYOffset`) calls methods that don't exist on this type and must be re-expressed. `SetContent` replaces the buffer + syncs + **clamps `TopRow` to `[0, maxTop]`** but bakes **no** scroll policy. textedit exposes `AtBottom() bool`, `GotoBottom() Model`, `ScrollOffset() int`, `SetScrollOffset(int) Model` (all in `ViewportState` terms) and the caller chooses:
- FileLoadedMsg (new file): **reset** `TopRow=0`, cursors=0, then `scrollToCursor`.
- chat-display append: **stick to bottom** iff `AtBottom()` before the set.
- same-file re-render: **preserve** offset (clamped).
Delete the dead `scrollPreservingAnchor` stub (`apply.go:192`).

**D6 — The title stays a dedicated `title.Model`; it does NOT become a textedit. The chat prompt DOES become a textedit.**
Converting the title to textedit would scatter its cohesive rename-debounce + placeholder/"Untitled" + revert-to-committed logic into the workspace, and would require gating off find/dictation/multicursor/softwrap/syntax (all nonsensical for a filename). The title keeps its own small component; it only **relocates** from an editor child to a **workspace sibling**. The chat prompt is a genuine multi-line input and becomes a `textedit` (replacing the hand-rolled `m.input string`). The `title/` package moves from `pkg/ui/components/editor/title/` to `pkg/ui/components/title/` (it is NOT deleted with the monolith).

**D7 — Inline images paint at ABSOLUTE terminal coordinates; markdownedit's `offsetY` must absorb the now-external title, and `emitImagePlacements` is the outermost `Update` wrapper.**
`buildInlineImagePlacements` (`image_integration.go:51-83`) does not render into the cell grid — it emits raw `\0337\033[row;colH…\0338` escapes via `tea.Raw`, computing `screenBase := m.offsetY + m.headerHeight()`. In the monolith the title was rendered *inside* `editor.View()` (`view.go:143`), so `headerHeight=1` and `offsetY=1` were self-consistent. After the split the title is a **workspace sibling rendered above** markdownedit, and `headerHeight=0`. For images to land on the right rows (and for mouse `dp.Row = msg.Y - offsetY - headerHeight` to be correct), **markdownedit's `offsetY` must equal the absolute screen row of its first content line = paneTop + top border + titleHeight.** The plan's old Blocker-5 claim ("workspace does NOT need to set this") and Step-3.6 claim ("No more `SetOffset`") are WRONG: positioning is still mandatory; its Y is bumped by the title height while `headerHeight` stays 0.
`headerHeight` (snapshot row budget / in-View math) and `offsetY` (absolute physical-screen row) were one number in the monolith and are now cleanly separated:
| Concern | Monolith | markdownedit after split |
|---|---|---|
| `contentHeight()` row budget | `height - headerHeight` | `height` (headerHeight=0) |
| mouse `dp.Row` | `Y - offsetY - headerHeight` | `Y - offsetY` (offsetY includes title) |
| image `screenBase` | `offsetY + headerHeight` | `offsetY` (offsetY includes title) |

Also: `emitImagePlacements` is **change-gated** (on `lastPlacementSeq`) and must wrap the *entire* `markdownedit.Update`, because the mouse-wheel/click arms mutate `viewport.TopRow` and thus the placement sequence. It is NOT confined to the `default` arm:
```go
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
    m, cmd := m.routeUpdate(msg)        // switch: mouse arms + placementTickMsg arm + default(delegate+afterContentChange)
    m, pcmd := m.emitImagePlacements()  // OUTERMOST, change-gated — runs for ALL msgs
    return m, tea.Batch(cmd, pcmd)
}
```
`placementTickMsg` is markdownedit-owned (image-specific), handled in `routeUpdate`. No infinite loop: after deferral `lastPlacementSeq == seq`, so the re-emit no-ops.

**D9 — `ReadOnly` is declarative gating + bypass-path guards + caret suppression + inert links — NOT "all mutation commands return early".**
Read-only gating is already fully declarative in the command `When` clauses, evaluated by the resolver against `ResolverContext.ReadOnly` (`parser.go:25-26`, `resolver.go:107`): `clipboard.copy`/`nav.*`/`multi.*`(selection)/`find.*` are `editorFocused` (work read-only); `clipboard.cut`/`paste`, `edit.*`, `edit_lines.*`, `history.undo/redo` are `editorFocused && !readOnly` (blocked). A blunt "all mutation returns early" would be redundant AND would wrongly kill `copy`. So `ReadOnly` is exactly:
1. **`ResolverContext.ReadOnly = m.readOnly`** (D4) — the `When` clauses do all resolver-routed gating automatically.
2. **Resolver-bypass mutation paths guard on `m.readOnly`** — the printable-char fallback (`editor.go:428-459`, direct `Execute("edit.insert-character")`), direct Undo/Redo (`editor.go:312-340`, via `key.Matches`), direct newline (`editor.go:369`), and paste handlers (`editor.go:149-159`, via `tea.PasteMsg`/`ClipboardMsg`) all early-return when read-only. These are the ONLY imperative read-only checks.
3. **View suppresses the insertion caret, keeps selection highlight** — pass empty `cursorOffsets` but populated `selections` to `applyOverlays` (currently both are gated on `m.focused` in `view.go:25-32`).
4. **handleMouseClick skips link resolution** — read-only *renders* links (spanToCellsStyled still styles `LinkRoleNavigable`) but they are **inert**: clicking does selection/cursor positioning only, never `resolveLinkClick`/`LinkClickedMsg`. Link click → file-open stays a **main-editor-only** behavior.

**Chat consequences (D9):** the chat display is a **focusable, selectable, copyable** read-only `markdownedit` with a **truthful absolute `Rect.Y`** (mouse drag-select needs hit-testing). Chat gains a **prompt↔display focus sub-state** so `Cmd+C` (`clipboard.copy`) resolves against the focused display. The display emits **no `LinkClickedMsg`** (inert links). "read-only" ≠ "non-interactive".

**D10 — Delete `m.scrollOff`; the display markdownedit owns conversation scroll via its own viewport.**
Once the conversation view is a real `markdownedit` (D9), a separate chat-level `m.scrollOff` is a second offset fighting the display's own `ViewportState` — the exact desync the component split eliminates. Delete `m.scrollOff`, its Up/Down/PgUp/PgDn handlers, and any hand-rolled history renderer. Keyboard scrollback = focus the display (`chatFocusDisplay`) + nav keys (nav.* works read-only); mouse wheel scrolls the display regardless of focus via coordinate hit-test.

**D11 — The title↔content focus transition is driven by a single `CursorAtTop()` query on textedit (promoted to markdownedit); the workspace owns the transition and consumes the Up keypress.**
The monolith's "Up at top of buffer → focus title" check (`editor.go:344-358`) iterates all cursors through buffer→syntax→wrap coords and verifies every cursor is on visual row 0 AND `viewport.TopRow == 0`. After the split this logic moves to the workspace, but the workspace MUST NOT import display/coords internals (§2.1, §10). So textedit exposes one intent-revealing query:
```go
func (m Model) CursorAtTop() bool   // all cursors on visual row 0 AND viewport.TopRow == 0
```
It is pure cursor+viewport+wrap geometry (no markdown), so it lives on textedit and is **promoted** to markdownedit (legal under D1 — leaf query). Workspace Up-handler:
```go
if m.focus == paneCenter && m.editor.Focused() && m.editor.CursorAtTop() {
    m.editor = m.editor.SetFocused(false)
    m.title  = m.title.SetFocused(true)
    return m, nil   // CONSUME — do not also forward Up (would move cursor / double-act)
}
// else fall through → forward Up to editor as normal navigation
```
Ordering matters (§3.4): this contextual check runs **before** the fallthrough-to-children forward. The return path is the existing title `FocusReturnMsg` (Down/Enter) — no new surface (D6).

**D12 — The editor knows only content+edits; the ENTIRE file/disk domain moves to the workspace package.**
`SetContent(content string)` is the correct shape — the editor never knows about a file on disk. The file/disk domain is split awkwardly today (editor owns `filePath`/`savedContentHash`/`activeSave`/`StartSave` + 9 file-message handlers; workspace already owns `origContent` merge-ancestor, `FileChangedOnDiskMsg`, `createFileCmd`, save-confirm UI, opentabs sync). D12 assigns the whole domain to the workspace.

*Editor (textedit/markdownedit) KEEPS — content/edit/render only (⚠️ D13 AMENDS: no dirty state at all):*
- `SetContent(content string)` / `Content() string`
- ~~`IsDirty()` / `MarkSaved()` / `savedContentHash`~~ — **REMOVED by D13.** Dirtiness lives in the workspace (`Content() != origContent`). Editor has no baseline.
- `Revision() uint64` — cheap monotonic mutation counter (D13); lets the workspace know when to recompute dirty. Content-domain, not disk.
- `SetDir(dir string)` — STAYS in markdownedit, reframed as "base dir for resolving relative image embeds" (`resolveEmbed`) — a rendering concern. No longer touches the breadcrumb.
- `ApplyMergeResult(ours, merged)` — content-level buffer replace, stays.

*Editor LOSES → workspace:* `filePath`/`FilePath()`/`SetFilePath()`/`BreadcrumbPath()`; `SaveIdentity`/`activeSave`/`StartSave()`/`startSaveRequest`; ALL file-message handling (`FileLoadedMsg`, `FileClosedMsg`, `FileSavedMsg`, `FileSaveErrorMsg`, `FileRenamedMsg`, `FileRenameErrorMsg`, `FileChangedOnDiskMsg`, `FileMergedMsg`, `UntitledRenameMsg`, `RenameRequestMsg`); command factories `LoadFileCmd`/`SaveFileCmd`/`FileRenameCmd`; the `file.save` command + `OperationSaveFile` path (**removed** — Cmd+S is a workspace-global per D4 that reads `Content()`, writes via the workspace's own Cmd, then updates `origContent` — D13, no `MarkSaved`).

*Location (fork → A chosen):* the file I/O Cmds + file message types live in the **workspace package** (`workspace_fileio.go`, ≤500 LoC), the already-existing file orchestrator. §2.4 producer-owns-message is satisfied because the workspace now produces them. NO new `document` package.

*Simplifies earlier decisions:* `BreadcrumbPath()` deleted (workspace sets breadcrumb from its own `filePath`); `markdownedit.StartSave()` deleted; Step-3.5 "route FileLoadedMsg to markdownedit then sync breadcrumb" becomes "workspace handles FileLoadedMsg → `SetContent` + own `filePath` + breadcrumb." The editor never sees a file message again. This also makes D4 fully coherent — no command-driven save competing with the global save.

**D13 — Dirtiness is NOT an editor concept. Workspace `origContent` is the SOLE baseline; dirty = `Content() != origContent`. (Amends D12.)**
Dirtiness is "differs from disk" — a disk concern — so it leaves the editor entirely (purer than D12's `IsDirty`/`MarkSaved`). The editor (textedit/markdownedit) has **no** `savedContentHash`, `IsDirty`, `MarkSaved`, or baseline — pure content+edits. The workspace holds `origContent []byte` as the single source of truth (it already is the merge ancestor) and derives dirty directly:
```go
func (m Model) isDirty() bool { return m.editor.Content() != string(m.origContent) }
```
- **No second baseline → no drift** (the reason to collapse): the D12 `syncBaseline`/two-field reconciliation is gone. There is exactly one writer of one field.
- **Data-integrity fix (Finding A) survives and is simpler.** On `FileSavedMsg` (RequestID match), set `m.origContent = m.activeSave.SavedContent` — the bytes actually written, NOT `m.editor.Content()`. Then `isDirty()` is automatically true iff the user edited mid-save. Setting it to `Content()` would corrupt the merge ancestor to a never-persisted state (silent data loss, §1.3). This is the highest-severity finding. The monolith already writes `msg.SavedContent` (workspace.go:690), but its stale-ack guard is **path-based** (`msg.Path == m.editor.FilePath()`) — a late ack from a prior, superseded same-file save still updates `origContent` while a newer save is in flight. The fix gates on the in-flight identity instead: `m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID` (Step 3.5).
- **`origContent` is set ONLY at disk-sync points**, always to the correct bytes: load (`msg.Content`), save-ack (`activeSave.SavedContent`), merge-accept (`result.Output`), merge-reject (disk content), file-create (`Content()` at creation), disk-change-noop (`msg.NewContent`).
- **`ContentChangedMsg` is DELETED** (Finding C): its `Path` is meaningless under D12 and it was sync-state Cmd ping-pong (§5.4). The editor exposes a cheap monotonic **`Revision() uint64`** (increments on buffer mutation — a content-domain counter, NOT a dirty/disk concept). The workspace recomputes `isDirty()` + opentabs mark + untitled-create only when `Revision()` changed since last pass, avoiding an O(n) `Content()` compare on non-mutating messages (mouse-move, image-tick).
- **Single save identity (Finding D):** `SaveIdentity{RequestID, SavedContent, InFlight}` is a workspace field minted once at save time; the pending close/switch follow-up keys off `m.activeSave.RequestID`.

**D14 — Test migration is a 4-bucket reclassification, not "fix references"; the save/dirty tests are REWRITTEN as workspace tests in Phase 3; generic helpers are duplicated.**
`editor_test.go` has 21 file/save/dirty references; `TestStaleSavesIgnored`/`TestDuplicateOutOfOrderSaves` + the file-load half of `TestEditorIntegration` poke `StartSave`/`activeSave`/`dirty`/`filePath`/`IsDirty` + feed `FileSavedMsg`/`FileLoadedMsg` — all workspace behavior now (D12/D13), un-compilable against the post-split editor. These two save tests are the regression guard for the exact data-loss class D13 fixes.

*Four buckets (~5,100 test LoC):*
| → package | Tests | Reaches |
|---|---|---|
| **textedit_test** | commands_edit, commands_nav, commands_multi, commands_history, commands_clipboard, selection_render, find_overlay, generic cell_test; editor_test: TestApplyOperation, TestPrintable*, TestCursor*, TestAltArrow*, TestScroll*, TestNavigation* | `m.cursors`/`m.buf` |
| **markdownedit_test** | highlight, link_render, commands_link_resolution, commands_mouse_link, commands_image, styled cell_test; editor_test: TestMarkdownReveal, TestRenderedLink*, TestHorizontalRule*, TestInlineCodeWrapping | `m.termCaps`/`m.images`/`m.highlighter` |
| **workspace_test (REWRITTEN)** | TestStaleSavesIgnored, TestDuplicateOutOfOrderSaves, file-load half of TestEditorIntegration, commands_rename_test, filename_validation_test | workspace `origContent`/`activeSave`/`filePath` |

*Save/dirty tests rewritten against D12/D13 and land in **Phase 3** (with the workspace rewrite), NOT Phase 5.3:* no `m.dirty=true` poke (gone — make dirty by editing the buffer; `TestDuplicateOutOfOrderSaves`'s own comment admits the flag-poke is dishonest); assert workspace state — `origContent`, `isDirty()`, `activeSave.RequestID` (read back; RequestID is non-deterministic per `editor.go:412`), and the **D13 invariant**: a stale/out-of-order ack does NOT clobber `origContent` or clear dirty; the matching ack sets `origContent == activeSave.SavedContent` (bytes written, not `Content()`). Phase 5.3 shrinks to: delete leftover editor test files + fix import paths.

*Helpers (fork → duplication):* `setCursor`, `runCmd`, `firstMsg`, `newTestEditor`/`newTestEditorFromNotation` are **duplicated** into each package's `testhelpers_test.go` (idiomatic Go — helpers don't cross package boundaries; a shared `editortest` pkg couldn't poke either type's unexported fields anyway, defeating `setCursor`). `writePNG`/`docEditor`/`kittyCaps` → markdownedit only; `docEditor` rewritten to `SetContent(content)` (1-arg, D12) + `SetDir(dir)`, replacing the dead 2-arg `SetContent(path, []byte)`.

**D15 — Dictation is NOT an editor feature. textedit exposes GENERIC programmatic-edit primitives; the dictation session is owned elsewhere.**
The monolith bakes a whole feature into the editor (`dictation.go`: `dictationState{active, startOff, totalLen}` + `StartDictation`/`ApplyDictationChunk`/`FinalizeDictation`/`CancelDictation`/`IsDictating`). By the D12/D13 principle one level deeper: the editor owns content + a generic mutation API; "dictation" composes on top.
- **DELETE editor `dictation.go` entirely** — no dictation state or methods on textedit/markdownedit.
- **textedit exposes generic primitives** (any programmatic producer — dictation, future LLM completion, snippet insertion — uses these), running the normal `applyOperation` pipeline so they are **undoable / history-tracked / coalescing**:
  - `ReplaceRange(start, end int, text string) Model` — the core primitive (insert = `ReplaceRange(off,off,t)`, delete = `ReplaceRange(s,e,"")`, append = `ReplaceRange(n,n,t)`).
  - `AppendText(text string) Model` — convenience.
  - `CursorOffset() int` — primary-cursor byte offset (lets a caller anchor a session).
- **markdownedit WRAPS `ReplaceRange` + `AppendText`** (delegate to textedit, then `afterContentChange()` — D1/D2; promotion would lose image re-expansion).
- **Read-only guards them** (D9 — they are resolver-bypass mutation paths; one guard on the generic primitive covers dictation and every future producer).
- **They bump `Revision()`** (D13 — workspace recomputes dirty via the Revision path; no `ContentChangedMsg`).

**D16 — Dictation is a GLOBAL workspace-owned component; the workspace routes chunks into the focused editor (value semantics); focus change ends the session.**
"Sends text into the focused editor" can't be literal — a component cannot hold/push into siblings (§1.1 value semantics). So it splits into owns-the-session vs owns-the-routing:
- **`dictation.Model` (new, `pkg/ui/components/dictation/`, workspace-owned, non-rendering):** owns the engine lifecycle (ctx/`cancel`, `dictCh`, `Ready`/`Listen`/`Partial`/`FinalTranscriptionMsg` — moved out of inline workspace handling), global `enabled` state, and the session (`startOff`, `appliedLen`). `Enable(startOff int) (Model, tea.Cmd)`, `Disable() (Model, tea.Cmd)`, `Update(msg) (Model, tea.Cmd)`, and `TakePendingEdit() (start, end int, text string, ok bool)` (computes `(startOff, startOff+appliedLen, accumulated)`, updates `appliedLen`). Renders nothing — the **footer** keeps the `^v` indicator, driven from `m.dict.Enabled()`.
- **The workspace owns the routing** (common ancestor of both targets), draining synchronously (§5.4, no self-message ping-pong):
  ```go
  m.dict, cmd = m.dict.Update(msg)
  if s, e, t, ok := m.dict.TakePendingEdit(); ok {
      switch m.focus {
      case paneCenter: m.editor = m.editor.ReplaceRange(s, e, t)   // markdownedit (D15)
      case paneChat:   m.chat   = m.chat.ApplyToPrompt(s, e, t)     // → prompt textedit.ReplaceRange
      }
  }
  ```
- **Enable anchors to the focused editor:** `m.dict = m.dict.Enable(focusedEditor.CursorOffset())`.
- **Focus change ends the session (policy i):** `startOff` is a byte offset in a specific buffer and is meaningless elsewhere, so changing focus auto-`Disable()`s dictation. The footer indicator makes this legible.
- **chat loses its dictation path entirely** — no `dictationPartial`/`SetDictationPartial`/`FinalizeDictation(text)`/`CancelDictation`; just a thin `ApplyToPrompt(s,e,t)` forwarding to `prompt.ReplaceRange`. (Supersedes Q15's "two ints per orchestrator" — one session in one component.)

**D8 — Position + size are set atomically via a SCOPED `SetRect(Rect)`, NOT a universal contract change.**
`offset` is a leaky-abstraction concession (absolute `tea.Raw` painting + mouse hit-testing), not general geometry — only 2 of 6 components have it (editor, filetree; the others are pure cell-renderers happy with `SetSize` alone, two reading only `width`). So the offset-bearing components (**textedit, markdownedit, filetree**) replace *both* `SetSize` and `SetOffset` with a single atomic `SetRect(r Rect) Model` where `type Rect struct{ X, Y, W, H int }` — they can't desync (D7 is exactly the desync bug). The cell-render-only components (**footer, breadcrumb, opentabs, chat**) keep `SetSize(w, h)`. CLAUDE.md §2.2 is updated: *"Cell-grid-only components expose `SetSize(w,h)`. Components doing absolute-screen positioning (mouse hit-test, `tea.Raw`) expose `SetRect(Rect)` instead — offset and size are co-dependent and must be set atomically."* `Rect.Y` doc carries the D7 invariant (absolute first-content-line row, includes title + top border).

## Component Ownership

```
workspace (pkg/ui/pages/workspace/)
├── title             title.Model        (dedicated component — owns rename-debounce/placeholder/revert; D6)
├── breadcrumb        breadcrumb.Model
├── editor            markdownedit.Model (MarkdownSync, images, links)
├── filetree          filetree.Model
├── opentabs          opentabs.Model
├── chat              chat.Model
│   ├── display       markdownedit.Model (read-only, conversation history)
│   └── prompt        textedit.Model     (PlainSync, dynamic height 3..50%)
├── dict              dictation.Model    (global engine+session; routes ReplaceRange to focused editor — D16; non-rendering)
└── footer            footer.Model
```

### Component Contracts

Each component exposes these concrete methods (value receivers, no interfaces):

```go
func New(keys keymap.Bindings, st styles.Styles, ...) Model
func (m Model) Init() tea.Cmd
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd)
func (m Model) View() string
func (m Model) SetRect(r Rect) Model    // textedit, markdownedit, filetree (offset-bearing) — D8
// func (m Model) SetSize(w, h int) Model   // footer, breadcrumb, opentabs, chat (cell-grid only) — D8
func (m Model) Height() int
func (m Model) SetFocused(focused bool) Model
```

**Sizing method is split by capability (D8).** Offset-bearing components (textedit, markdownedit, filetree) expose `SetRect(Rect)` which sets position+size atomically (`type Rect struct{ X, Y, W, H int }`). Cell-grid-only components (footer, breadcrumb, opentabs, chat) keep `SetSize(w, h)`. `Rect.Y` is the absolute screen row of the component's first content line — the workspace MUST include the title line and top border (D7).

**recalcLayout() is an internal helper, NOT part of the public contract.** It is called only from `Update()` when `tea.WindowSizeMsg` arrives or when structural changes occur (pane toggled, tab added/removed). It is NOT called from `Init()` — at `Init()` time `m.totalWidth` and `m.totalHeight` are zero, so `SetSize(0, 0)` on children is a no-op that wastes a pass.

**Child components (textedit, markdownedit) do NOT handle `tea.WindowSizeMsg`.** Only the workspace page receives window resize and distributes via `SetSize`. A child that handles `tea.WindowSizeMsg` directly would ignore its allocated dimensions from the parent, creating a dimension conflict.

### Message Ownership & Flow

Messages are defined in the **producer's** package. Per **D12** the entire file/disk domain (I/O Cmds + file messages) lives in the **workspace package** — the editor never produces or consumes a file message:

| Message | Defined in | Consumed by |
|---------|-----------|-------------|
| `filetree.FileSelectedMsg{Path}` | `components/filetree` | `workspace` → issues its own `loadFileCmd(ctx, msg.Path)` → `SetContent` on editor |
| `workspace.FileLoadedMsg{Path, Content}` | `pages/workspace` (D12) | `workspace` → `editor.SetContent(string(Content))`, sets own `filePath`, breadcrumb, opentabs |
| `workspace.FileSavedMsg{Path}` | `pages/workspace` (D12) | `workspace` → set `origContent = activeSave.SavedContent`, recompute `isDirty()`, opentabs `MarkClean`/`MarkDirty`, breadcrumb (NO editor call — `MarkSaved` does not exist; D13) |
| `markdownedit.LinkClickedMsg{Path}` | `components/markdownedit` | `workspace` → opens file (main editor only — D9) |
| `footer.ConfirmQuitMsg` | `components/footer` | `workspace` → calls `tea.Quit` |

**Cmd factory rule (D12):** the file I/O Cmds (`loadFileCmd`/`saveFileCmd`/`fileRenameCmd`) are defined in the **workspace package** (`workspace_fileio.go`), not the editor. Per §6.2 they accept `context.Context` for cancellation:

```go
func loadFileCmd(ctx context.Context, path string) tea.Cmd   // workspace package
```

Without context cancellation, a slow file load cannot be interrupted by navigation (e.g., user switches tabs while a file is loading). The editor exposes only `SetContent(string)`/`Content()`/`Revision()` — the workspace owns the path, the I/O, AND dirtiness (`Content() != origContent` — D13).

### Scroll Preservation (§5.7) — see D5

**The viewport is a custom `ViewportState{TopRow, ScrollCol}`, NOT bubbles `viewport.Model`.** The §5.7 rule in CLAUDE.md is written for bubbles viewport (`AtBottom()/YOffset/GotoBottom/SetYOffset`); those methods do not exist here and the pattern must be re-expressed in `ViewportState` terms. `scrollPreservingAnchor` (`apply.go:192`) is a dead stub — delete it.

**Scroll policy is chosen by the caller, not baked into `SetContent`** (D5). `SetContent` replaces the buffer, syncs, and clamps `TopRow` into `[0, maxTop]` (fixes the latent staleness bug where loading a short file leaves the viewport scrolled into the void). It applies **no** stick-to-bottom/preserve policy.

textedit exposes the primitives (all `ViewportState`-native):
```go
func (m Model) AtBottom() bool             // TopRow >= TotalRows - contentHeight
func (m Model) GotoBottom() Model          // TopRow = max(0, TotalRows - contentHeight)
func (m Model) ScrollOffset() int          // m.viewport.TopRow
func (m Model) SetScrollOffset(int) Model  // clamps to [0, maxTop]
```

Callers choose the policy:
- **FileLoadedMsg (new file)** — reset: `TopRow=0`, cursors=0, then `scrollToCursor`. No preservation.
- **chat-display append** — stick to bottom iff at bottom: `wasBottom := disp.AtBottom(); disp = disp.SetContent(...); if wasBottom { disp = disp.GotoBottom() }`.
- **same-file re-render** — preserve: capture `ScrollOffset()`, `SetContent`, restore via `SetScrollOffset`.

Implementation lands in Step 1.7 (textedit) and the chat/markdownedit callers.

### Cmd Batching Convention

Per §5.5, ALL `Update()` methods accumulate child `tea.Cmd` into a slice and return via `tea.Batch`:

```go
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
    var cmds []tea.Cmd
    switch msg := msg.(type) {
    // ... case arms that append to cmds
    }
    return m, tea.Batch(cmds...)
}
```

Some arms append to `cmds`, others do nothing. The single return at the bottom always uses `tea.Batch(cmds...)`. No arm returns `m, nil` or `m, cmd` directly — that pattern is prohibited as it creates inconsistent delivery timing.

## New API Surface

### textedit (pkg/ui/components/textedit/)

**Constructor:**
```go
func New(keys keymap.Bindings, st styles.Styles, opts ...Option) Model
```
Options: `WithSyncFunc`, `WithSanitizeFunc`, `WithSingleLine`, `WithReadOnly`, `WithPaddingTop(int)`

**Header height handling (Blocker 4, 5):**
textedit does NOT own a title component. Instead, it accepts an optional header height via `WithPaddingTop(n)`. This value is subtracted from allocated dimensions to compute the content area. When not specified, headerHeight defaults to 0. markdownedit passes 0 (no title). The workspace passes the title's 1-line height when computing content dimensions for the markdownedit instance.

**New methods** (⚠️ `NaturalContentHeight` and `Revision` are **BUILD from scratch — not in the monolith**; `Content`/`SetContent` shapes are new even though a same-named monolith method exists):
- `SetReadOnly(bool) Model` — gates mutation commands, hides cursor; also drives `ResolverContext.ReadOnly` (D4)
- `NaturalContentHeight(width int) int` — **BUILD new.** natural visual height of content at given width (excludes header)
- `Content() string` — returns buffer text
- `SetContent(content string) Model` — replaces buffer content, resets cursors, syncs display, clamps `TopRow` to `[0, maxTop]`. **No baked scroll policy** — caller chooses (D5). **No path/file/dirty awareness** (D12, D13). **Reduced new method, NOT migrated:** the monolith's `SetContent(path string, content []byte)` (editor.go:566) also sets `m.filePath`/`m.savedContentHash`/`m.dirty=false`; those responsibilities move to the workspace's `FileLoadedMsg` handler (Step 3.5). Monolith callers (`workspace.go:1242` production; `workspace_test.go:1146/1170/1197` tests) keep calling the 2-arg monolith version until Phase 3/5 (Freeze invariant); the 1-arg form is used only by the rewritten workspace and bucket-1/3 tests.
- `Revision() uint64` — **BUILD new.** monotonic buffer-mutation counter (D13); workspace uses it to gate dirty recomputation. NO `IsDirty`/`MarkSaved` (D13 — dirtiness is workspace-owned; note `MarkSaved` never existed in the monolith — the machinery being removed is `dirty`/`IsDirty`/`savedContentHash`/`SetDirtyForTest`).
- `CursorOffsets() map[int]bool` — query: cursor byte offsets for parent overlay rendering
- `Selections() []selInterval` — query: selection intervals for parent overlay rendering
- `Focused() bool` — query: is this component focused?
- `WantsModalInput() bool` — query: is find overlay open?

**Generic programmatic-edit primitives (D15) — replace the deleted dictation methods. ⚠️ BUILD from scratch — `ReplaceRange`/`AppendText`/`CursorOffset` do not exist in the monolith (verified by grep):**
- `ReplaceRange(start, end int, text string) Model` — core primitive (undoable, history-tracked); insert/delete/append all expressed via it
- `AppendText(text string) Model` — convenience
- `CursorOffset() int` — primary-cursor byte offset (lets a caller anchor a session)
- (NO `StartDictation`/`ApplyDictationChunk`/`FinalizeDictation`/`CancelDictation`/`IsDictating` — dictation is a global component, D16. Read-only guards ReplaceRange/AppendText, D9; they bump Revision, D13.)

**Exported seams for markdownedit composition (D1, D2, D3, D5).** ⚠️ **BUILD from scratch — none of these exist in the monolith** (verified by grep: `Snapshot`/`SetSnapshot`/`ScrollToCursor`/`ContentHeight`/`AtBottom`/`GotoBottom`/`ScrollOffset`/`SetScrollOffset`/`Mouse*`/`CursorAtTop` are all net-new). These are the public surface markdownedit's explicit `Update` wrapper, `afterContentChange()` tail, and mouse handlers drive (markdownedit must NOT rely on method promotion for any of this):
- Snapshot/scroll: `Snapshot() display.DisplaySnapshot`, `SetSnapshot(display.DisplaySnapshot) Model`, `ScrollToCursor() Model`, `ContentHeight() int`
- Scroll policy: `AtBottom() bool`, `GotoBottom() Model`, `ScrollOffset() int`, `SetScrollOffset(int) Model`
- Low-level mouse: `MousePositionCursor(offset int) Model`, `MouseExtendSelection(offset int) Model`, `MouseSelectWord(offset int) Model`, `MouseSelectLine(line int) Model`, `MouseAddCursor(offset int) Model`, `DisplayToBuffer(...) coords.BufferPoint`
- Snapshot accessors for mouse/link: `SyntaxSnap() display.SyntaxSnapshot`, `WrapSnap() display.WrapSnapshot`, `Viewport() ViewportState`
- Focus transition: `CursorAtTop() bool` — all cursors on visual row 0 AND viewport at top; backs the workspace title↔content transition (D11)

**Methods carried from current editor:**
- `Init() tea.Cmd`
- `Update(tea.Msg) (Model, tea.Cmd)` — accumulates Cmds into `var cmds []tea.Cmd`; returns `tea.Batch(cmds...)` at bottom
- `View() string`
- `SetRect(r Rect) Model` — atomic position+size for offset-bearing components (replaces SetSize+SetOffset; D8). `type Rect struct{ X, Y, W, H int }`
- `Height() int`
- `SetFocused(bool) Model`

**Commands registered by textedit (into the shared union registry — D4):**
edit.*, nav.*, multi.*, clipboard.*, history.* — plus **find.* stubs and mouse no-ops** (registered only so `app.go` startup verification finds every keybind's command name; the overlay and mouse are driven by direct methods, not these registrations). The registry is shared across every instance; markdown-only commands resolving in a plain-textedit context are silent no-ops. `ResolverContext.ReadOnly` is set from `m.readOnly`.

**Find overlay (Blocker 8):**
FindOverlay stays in textedit as internal state. The overlay state machine (open/close/consumeKey/Visible) is a UI concern, not a command. Find commands are excluded from the command registry (stubs/no-ops). The overlay is opened/closed via direct method calls from Update(), not via registered commands.

**Dictation (D15, D16): NOT in textedit.**
The editor's `dictation.go` is deleted. textedit provides only the generic `ReplaceRange`/`AppendText` primitives; the dictation engine+session is a global `dictation.Model` component (D16) and the workspace routes chunks into the focused editor via `ReplaceRange`.

### markdownedit (pkg/ui/components/markdownedit/)

```go
func New(keys keymap.Bindings, st styles.Styles, termCaps terminal.TermCaps, opts ...Option) Model
```

Embeds `textedit.Model` by value (for field/query reuse), but **does NOT rely on method promotion for the edit/sync/render cycle** (D1). markdownedit owns its own `Update`, `syncDisplay`, `dispatchOperation`, View, and mouse handlers; promotion is used only for generic leaf queries (`Content`, `CursorOffsets`, `Selections`, `Focused`, `WantsModalInput`, `SetFocused`, `SetSize`, `Height`). Adds:
- MarkdownSync (SyncFunc with goldmark + Chroma)
- Image lifecycle (discover, load, allocate, place, animate, cleanup)
- Link click resolution (LinkClickedMsg)
- Markdown-specific cell styling (spanToCellsStyled)
- DispatchOperation (command result handling including save)

**Explicit `Update` wrapper (D1, D2, D3, D7):** `emitImagePlacements` is the OUTERMOST layer over the whole Update (mouse arms change `viewport.TopRow` → placement seq), exactly as the monolith (`editor.go:136-144`):
```go
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
    m, cmd := m.routeUpdate(msg)
    m, pcmd := m.emitImagePlacements()   // change-gated on lastPlacementSeq; runs for ALL msgs — D7
    return m, tea.Batch(cmd, pcmd)
}

func (m Model) routeUpdate(msg tea.Msg) (Model, tea.Cmd) {
    switch msg := msg.(type) {
    case tea.MouseClickMsg:   return m.handleMouseClick(msg, now)   // link → emit; else textedit low-level helpers
    case tea.MouseMotionMsg:  return m.handleMouseMotion(msg)
    case tea.MouseWheelMsg:   return m.handleMouseWheel(msg)        // scroll + image arm
    case placementTickMsg:    return m.handlePlacementTick()        // image-owned, deferred tea.Raw
    default:
        var cmd tea.Cmd
        m.Model, cmd = m.Model.Update(msg)   // textedit: edit + generic sync + PROVISIONAL scroll
        m, tcmd := m.afterContentChange()     // expand images, FINAL scroll, discover, collapse, arm
        return m, tea.Batch(cmd, tcmd)
    }
}
```

**`afterContentChange()` tail (D2)** — runs on the image-expanded snapshot, every pass after delegation:
`expandImageRows → ScrollToCursor (absolute, re-run) → re-clamp scroll (no delta) → discoverNewImages → detectImageCollapse → armImageTicks`.

**Commands registered by markdownedit:**
image.*, link.* — **NOT file.\*** (D12: the `file.save` command is removed; save is a workspace-global Cmd+S. Rename is workspace-orchestrated via the title's `RenameRequestMsg`.)

**New methods (D12 — NO file/path/save methods; those are workspace-owned):**
- `SetDir(dir string) Model` — base directory for resolving relative image embeds (rendering concern, not disk)
- `DispatchOperation(result command.Result, cmdName string) (Model, tea.Cmd)` — handle command results (NO OperationSaveFile — removed per D12)
- `RefreshImagesAfterLayoutChange() (Model, tea.Cmd)` — retransmit images after resize
- `DeleteAllImagesCmd() tea.Cmd` — cleanup inline images on exit
- `ApplyMergeResult(ours, merged []byte) Model` — content-level buffer replace for 3-way merge
- (`SetContent`/`Content`/`Revision` promoted from textedit; `IsDirty`/`MarkSaved`/`FilePath`/`StartSave`/`BreadcrumbPath` **deleted** — D12, D13. Dirtiness is workspace-owned.)

**Cell rendering (Blocker 2, 10):**
markdownedit owns `spanToCellsStyled` which dispatches on markdown span kinds (TokenHeading, TokenBold, TokenCodeFence, etc.). textedit has a simpler `spanToCells` that handles revealed/plain spans only. markdownedit's `View()` re-implements the render loop using `spanToCellsStyled` (it does NOT call `textedit.View()`); both build on textedit's shared generic `Cell` helpers (`sliceCells`/`applyOverlays`/`cellsToString`).

## File Mapping

### textedit package files
| Source file | Fate |
|---|---|
| textedit.go (new scaffold) | New |
| commands_registry.go | Migrate — edit, nav, multi, clipboard, history, mouse (low-level) |
| commands_edit.go | Migrate |
| commands_edit_lines.go | Migrate |
| commands_edit_lines_indent.go | Migrate |
| commands_edit_lines_multi.go | Migrate |
| commands_nav.go | Migrate |
| commands_nav_gen.go | Migrate |
| commands_multi.go | Migrate |
| commands_history.go | Migrate |
| commands_clipboard.go | Migrate |
| commands.go | **Move to workspace package (D12)** — `loadFileCmd(ctx, path)`, `saveFileCmd`, `fileRenameCmd`, `SaveRequest` become workspace-owned file I/O in `workspace_fileio.go`. NOT in textedit/markdownedit. LoadFileCmd accepts context for cancellation (§6.2). |
| commands_mouse.go | Migrate — low-level mouse helpers only (mousePositionCursor, mouseExtendSelection, mouseSelectWord, mouseSelectLine, mouseAddCursor, displayToBuffer, wordBoundaryLeft/Right). NO link resolution. |
| commands_find.go | Migrate — stubs only |
| commands_file.go | **Delete (D12)** — the `file.save` command + OperationSaveFile path are removed. Save is a workspace-global Cmd+S; rename is workspace-orchestrated. Not migrated to any editor package. |
| find_overlay.go | Migrate — FindOverlay struct, open/close/consumeKey/Visible |
| apply.go | Split: applyOperation, applyUndo, applyRedo → textedit. syncDisplay (lines 124-144, sans image expansion) → textedit. dispatchOperation lines 198-256 (sans OperationSaveFile) → textedit. |
| cell.go | Split: spanToCells, revealedSpanToCells, renderedSpanToCells, applyOverlays, sliceCells, cellsToString, cellEffectiveStyle, stylesEqual → textedit. spanToCellsStyled → markdownedit. codeFenceSpanToCells → markdownedit. taskListSpanToCells → markdownedit. tableSpanToCells → markdownedit. headingStyle → markdownedit. tableRoleStyle → markdownedit. mergeInlineStyle → markdownedit (used exclusively by tableSpanToCells; lateral dependency rule §10 requires it to follow its only caller). |
| render_code.go | Split: selInterval, isInSelection → textedit (used by applyOverlays). classToStyle → markdownedit (used by codeFenceSpanToCells). |
| history.go | Migrate |
| editor.go | Migrate — Model struct, New, Init, **SetRect (replaces SetSize+SetOffset — D8)**, Height, SetFocused, Content, CursorInfo, SetDir, SetHighlighter, UndoForTest, RedoForTest, ApplyMergeResult, PreferredWidth, isPrintableChar, firstRune. **BUILD new (not in monolith):** `Revision (D13)`, `SetContent(content string)` (reduced 1-arg form; monolith has `SetContent(path, []byte)` at editor.go:566), and the exported seams (D2/D3/D5/D11): Snapshot/SetSnapshot/ScrollToCursor/ContentHeight, AtBottom/GotoBottom/ScrollOffset/SetScrollOffset, Mouse* helpers, SyntaxSnap/WrapSnap/Viewport, CursorAtTop. **DELETED (→ workspace, D12/D13):** filePath, FilePath, SetFilePath, BreadcrumbPath, StartSave, `SaveIdentity` (monolith `{Path, RequestID, ContentHash, InFlight}`; workspace re-creates as `{RequestID, SavedContent, InFlight}` — NOT a copy), activeSave, savedContentHash, **IsDirty, dirty, SetDirtyForTest (D13 — dirtiness is workspace-owned, no editor baseline; note `MarkSaved` never existed)**, TitleText/SetTitle/BreadcrumbView (D6). |
| messages.go | Split (D12, D13): all FILE messages → **workspace package** (FileLoadedMsg, FileLoadErrorMsg, FileClosedMsg, FileSavedMsg, FileSaveErrorMsg, FileRenamedMsg, FileRenameErrorMsg, UntitledRenameMsg, FileChangedOnDiskMsg, FileMergedMsg). ClipboardContentMsg → textedit (paste = edit concern). **ContentChangedMsg → DELETED (D13)** — workspace computes dirty via `Content() != origContent`, gated by `Revision()`. LinkClickedMsg → markdownedit. |
| commands_image.go | Split: ImageConfig → markdownedit. |
| image_integration.go | Split: discoverNewImages, clearImages, updateImages, emitImagePlacements, buildInlineImagePlacements, retransmitImagesCmd, armImageTicks, detectImageCollapse, DeleteAllImagesCmd, resizeImages, imageDimsFor, imageKittyCapable, imageInlineCapable, imageCapable, imageIDFor → markdownedit. (imageKittyCapable/imageInlineCapable/imageCapable access m.termCaps; imageIDFor accesses m.images — both markdown-specific.) placementTickMsg → markdownedit. |
| image_allocator.go | Migrate to markdownedit — imageIDAllocator (image-specific) |
| highlight.go | Migrate to markdownedit — ChromaHighlighter, CodeHighlighter, HighlightSpan (chroma-based highlighting) |
| render_image.go | Migrate to markdownedit — imagePlaceholderCells, idToColor (uses imageIDFor which accesses m.images — markdown-specific) |
| filename_validation.go | **Move to the workspace package (`workspace_fileio.go`) — NOT textedit.** breadcrumb's validator param is currently `_` (ignored, breadcrumb.go:23), so `validateFilename` has **no effective production caller** today — `editor.go:120` passes it to `breadcrumb.New(st, validateFilename)`, but `breadcrumb.go:23` discards it via `_ func(string) error`. When the monolith is deleted, that call site goes with it; the workspace calls `validateFilename` directly in its rename path. Under D12 rename is workspace-orchestrated (title `RenameRequestMsg` → workspace `fileRenameCmd`); the workspace should call `validateFilename` on the requested name before issuing the rename Cmd, surfacing the error per §1.3 (no silent fallback). Placing it in textedit would force a banned lateral `breadcrumb → textedit` import (§10). `filename_validation_test.go` → bucket-3 `package workspace` (D14). |
| view.go | Split: View → split (see below). |
| dictation.go | **Delete from editor (D15/D16).** Generic `ReplaceRange`/`AppendText` primitives go to textedit (editor.go); the dictation engine stays in `pkg/dictation`; a new `pkg/ui/components/dictation/` component owns engine+session; workspace routes chunks. NOT migrated as-is. |

### markdownedit package files
| Source file | Fate |
|---|---|
| markdownedit.go (new scaffold) | New — embeds textedit.Model, adds markdown fields |
| commands_registry.go | Migrate — image, link, file commands + dispatchOperation |
| cell.go | Migrate: spanToCellsStyled, codeFenceSpanToCells, taskListSpanToCells, tableSpanToCells, headingStyle, tableRoleStyle, mergeInlineStyle |
| apply.go | Migrate: syncDisplay wrapper (lines 145-147 image expansion), dispatchOperation save handling (lines 259-268), startSaveRequest |
| image_integration.go | Migrate: full image pipeline |
| commands_link_resolution.go | Migrate: resolveLinkClick, handleMouseClick link branch |
| find_overlay.go | Not migrated — stays in textedit |
| commands_mouse.go | Migrate: handleMouseClick link branch (resolveLinkClick), handleMouseClick non-link branch delegates to textedit. displayToBuffer → textedit (already migrated). |
| messages.go | Migrate: LinkClickedMsg only (file messages → workspace, D12) |
| commands_image.go | Migrate: ImageConfig |
| commands_file.go | **Deleted, not migrated (D12)** — file.save command removed; save is workspace-global |
| image_allocator.go | Migrate: imageIDAllocator |
| highlight.go | Migrate: ChromaHighlighter, CodeHighlighter, HighlightSpan |
| render_code.go | Migrate: classToStyle |
| render_image.go | Migrate: imagePlaceholderCells, idToColor |
| filename_validation.go | **Not migrated to markdownedit** — moves to the workspace package (see textedit table; validateFilename is a workspace rename concern, D12). |
| view.go | Migrate: full View with single-loop image+text rendering |

## Rendering Architecture (addresses Blockers 1, 2, 5, 6)

### textedit.View() — generic cell rendering
```go
func (m Model) View() string {
    // 1. Slice snapshot lines (vertical)
    // 2. For each line: spanToCells (generic, revealed-only) → sliceCells → applyOverlays → cellsToString
    // 3. Join lines, apply MaxWidth/MaxHeight
    // No image handling, no title, no markdown-specific styling
}
```

### markdownedit.View() — single-loop image+text with markdown styling
```go
func (m Model) View() string {
    // Single loop over snapshot lines (same as current editor/view.go:39-111):
    for each line l:
        if l.ImagePath != "" && imageCapable:
            // Render image placeholder / iTerm2 space cells
        else:
            // For each span: spanToCellsStyled (markdown dispatch) → cells
            // EOL cursor, sliceCells, applyOverlays, cellsToString
    // Join lines, apply MaxWidth/MaxHeight (NO title area — workspace owns title as a sibling, D6)
}
```

This preserves the existing rendering semantics where images replace entire lines within the content stream. markdownedit.View() does NOT call textedit.View() as a sub-view — it re-implements the rendering loop with markdown-aware cell generation. textedit.View() is a separate, simpler implementation used by title and chat prompt instances.

### Cell rendering split (Blocker 2, 10)
- **textedit.cell.go**: `spanToCells`, `revealedSpanToCells`, `renderedSpanToCells`, `applyOverlays`, `sliceCells`, `cellsToString`, `cellEffectiveStyle`, `stylesEqual` — all generic, no markdown dispatch.
- **markdownedit.cell.go**: `spanToCellsStyled` (full dispatch on TokenHeading, TokenBold, TokenCodeFence, TokenItalic, TokenStrikethrough, TokenBlockquote, TokenHorizontalRule, TokenTag, TokenListMarker, TokenTaskList, TokenTable, LinkRoleImage, LinkRoleNavigable), `codeFenceSpanToCells` (uses Chroma highlighter), `taskListSpanToCells`, `tableSpanToCells`, `headingStyle`, `tableRoleStyle`, `mergeInlineStyle` (used exclusively by `tableSpanToCells`; placing it in textedit would force a cross-package call from markdownedit into textedit — a lateral dependency banned by §10).

## Mouse Handling Architecture (Blocker 1, 5)

### textedit.mouse.go — low-level mouse helpers
- `displayToBuffer(dp, vp, ws, ss)` — display→wrap→syntax→buffer conversion
- `mousePositionCursor(offset)`, `mouseExtendSelection(offset)`, `mouseSelectWord(offset)`, `mouseSelectLine(line)`, `mouseAddCursor(offset)` — cursor manipulation
- `wordBoundaryLeft/Right(b, offset)` — word detection
- `prevRuneOffsetBuf(b, offset)` — rune boundary

### markdownedit.mouse.go — link-aware mouse handling (D3)
markdownedit **intercepts mouse messages in its own `Update` switch** (it must not forward them to `textedit.Update`, which would position the cursor on a link click). Handlers call textedit's *exported low-level helpers* (`MousePositionCursor`, `MouseExtendSelection`, …, `DisplayToBuffer`) plus accessors (`SyntaxSnap()`, `WrapSnap()`, `Viewport()`) — never `textedit.Update`.
- `handleMouseClick(msg, now)` — full handler:
  1. Check focused, left button
  2. Compute display point (subtract headerHeight from msg.Y)
  3. Skip image-reserved rows (check snapshot.Lines)
  4. Call textedit's `DisplayToBuffer` to get buffer point and offset
  5. **If NOT read-only: call resolveLinkClick(bp, offset)** using markdownedit's own `syntaxSnap` (via `SyntaxSnap()`). Read-only skips this — links are inert (D9).
  6. If link (and not read-only): emit LinkClickedMsg (cursor NOT moved)
  7. Otherwise (no link, or read-only): call textedit's exported low-level cursor helpers (selection/positioning), then run `afterContentChange()` (cheap — click doesn't mutate, so effectively re-scroll/arm)
- `handleMouseMotion(msg)` — drag selection (uses textedit's `DisplayToBuffer` + cursor helpers)
- `handleMouseWheel(msg)` — scroll + image arm

**textedit keeps its own `MouseClickMsg` arm** in `textedit.Update` for standalone instances (chat prompt) that have no links and need plain click-to-position (D3).

### headerHeight vs offsetY in mouse + image coordinates (Blocker 5, D7)
`handleMouseClick`/`handleMouseMotion` compute `dp.Row = msg.Y - m.offsetY - m.headerHeight()`; image placement computes `screenBase = m.offsetY + m.headerHeight()`. markdownedit sets `headerHeight = 0` (no title in its own View). **But the workspace MUST still position markdownedit (via `SetRect`), and `Rect.Y` must absorb the title line** = paneTop + top border + titleHeight (D7). headerHeight=0 is correct *because* the title row is folded into `offsetY`, not because positioning is unnecessary. Getting `offsetY` wrong by the title height smears images one row off and mis-hits clicks.

## Sync Pipeline Architecture (Blocker 3)

### textedit.syncDisplay() — generic sync (apply.go lines 124-144)
```go
func (m Model) syncDisplay() Model {
    // Lines 124-136: syntaxMap setup
    // Lines 137-141: syntaxMap.Sync or SyncNoReveal
    // Lines 142-143: wrapMap.Sync
    // Line 144: display.BuildSnapshot
    // Line 145: display.ExpandTableRows
    // NO ExpandImageRows — that's markdown-specific
}
```

### markdownedit image expansion — driven by `afterContentChange()`, NOT a promoted override (D1, D2)
Because promotion is static, textedit's internal `m.syncDisplay()` calls always run textedit's (image-free) version. markdownedit therefore does NOT define a `syncDisplay` that textedit will magically call. Instead, after delegating to `textedit.Update`, markdownedit re-expands the freshly-built snapshot in its `afterContentChange()` tail:
```go
func (m Model) afterContentChange() (Model, tea.Cmd) {
    snap := m.Model.Snapshot()
    snap = display.ExpandImageRows(snap, m.imageDimsFor) // m.images lives on markdownedit
    m.Model = m.Model.SetSnapshot(snap)
    m.Model = m.Model.ScrollToCursor()    // FINAL absolute scroll on expanded snapshot
    m = m.reclampScroll()                 // re-clamp maxTop (no delta) — D2
    var dcmd, acmd tea.Cmd
    m, dcmd = m.discoverNewImages()
    m, _ = m.detectImageCollapse()
    m, acmd = m.armImageTicks()
    return m, tea.Batch(dcmd, acmd)
}
```
`imageDimsFor` stays in markdownedit (accesses `m.images`).

### dispatchOperation (Blocker 9, D2, D12)
- **textedit.dispatchOperation** (apply.go lines 198-256): handles OperationHistory, OperationScroll, OperationNone, and general apply+sync+*provisional* scrollToCursor. Does NOT handle image expansion/discovery/collapse (markdown-specific, run in `afterContentChange`).
- **No OperationSaveFile path anywhere (D12).** The `file.save` command is removed; neither textedit nor markdownedit handles save. Save is a workspace-global Cmd+S that reads `editor.Content()`, writes via the workspace's `saveFileCmd`, and on `FileSavedMsg` updates `origContent` to the bytes written (D13). The editor has no save-ack method (`MarkSaved` does not exist). markdownedit does not wrap dispatchOperation via promotion; the markdown tail runs uniformly in `afterContentChange()`.

## Header Height Architecture (Blocker 4, 5)

### Problem
`contentHeight()` returns `m.height - m.headerHeight()` where `headerHeight()` returns `m.title.Height()`. Multiple callers (scrollToCursor, buildInlineImagePlacements, handleMouseClick) depend on this. When title moves to workspace, textedit/markdownedit no longer has `m.title`.

### Solution
textedit accepts `headerHeight int` via `WithPaddingTop(n)` constructor option. All internal calculations use this stored value instead of `m.title.Height()`.

- **markdownedit**: headerHeight = 0 (no title — workspace owns title as sibling)
- **title textedit instance**: headerHeight = 0 (no header above title)
- **chat prompt textedit instance**: headerHeight = 0

**contentHeight(width int) → naturalContentHeight(width int):**
textedit exposes `NaturalContentHeight(width int)` which returns the visual height of content at the given width. This is a query method, not a dimension calculation. The parent (workspace) computes: `contentH = totalH - footer.Height() - titleHeight`. Then passes `contentH` as the allocated height to markdownedit via `SetRect(Rect{..., H: contentH})` (D8; `Rect.Y` absorbs the title row per D7).

**markdownedit.contentHeight()** returns `m.height` (the allocated height already excludes header).

## Instance Configurations

### Title (title.Model, workspace-owned) — D6
```
title.New("Untitled", styles)   // dedicated component, NOT textedit
```
Keeps its own rename-debounce (`RenameRequestMsg`), placeholder/"Untitled" semantics, revert-to-committed (Escape), and focus transitions (Up no-op, Down/Enter → `FocusReturnMsg`). Relocated from `pkg/ui/components/editor/title/` to `pkg/ui/components/title/`.

### Chat prompt (textedit, chat-owned)
```
textedit.New(keys, styles,
    WithSyncFunc(PlainSync),
    WithPaddingTop(0),
)
```
Dynamic height: `NaturalContentHeight(width)` returns natural height. Parent clamps to `[3, 50%]`.

### Chat display (markdownedit, chat-owned, read-only — D9)
```
markdownedit.New(keys, styles, termCaps).SetReadOnly(true)
```
Content rebuilt from `chat.messages[]` on each new message. Read-only per D9: **focusable, selectable, copyable**; renders links but they are **inert** (no LinkClickedMsg, no file-open — that stays main-editor-only). Insertion caret suppressed, selection highlight kept. Needs a truthful absolute `Rect.Y` (mouse drag-select hit-testing).

### Main editor (markdownedit, workspace-owned)
```
markdownedit.New(keys, styles, termCaps)
```
No special options — default MarkdownSync, multiline, editable, headerHeight=0.

## Phases

> **Build-green invariant (applies to all phases).** Phases 1–2 create `textedit`/`markdownedit` by **copying** from the monolith; they do **not** modify any file under `pkg/ui/components/editor/`. The monolith stays compiling and `workspace.go` keeps importing `editor` unchanged (still calling `editor.LoadFileCmd`, `SetContent(path, []byte)`, `TitleText()`, `BreadcrumbView()`, `IsDirty()`) until Phase 3. New signatures (`SetContent(content string)`, `loadFileCmd(ctx, path)`, no `dirty`/`IsDirty`/`savedContentHash`, plus net-new `Revision`) live only in the new packages. The workspace switchover from `editor.Model` to `markdownedit.Model` happens entirely in Phase 3; the monolith and its tests are deleted in Phase 5. There is **no atomic signature change to the monolith** and **no pre-Phase-1 deletion of monolith fields**. Two packages with overlapping copied code compile fine; `app.go` is untouched until Phase 5.2, so there is no double command-registration.

### Phase 1: Create textedit package (extract from monolith)

**Step 1.1 — Create package scaffold.**
- Create `pkg/ui/components/textedit/textedit.go`
- Copy Model struct from current editor, drop markdown-specific fields
- Fields to KEEP: `buf`, `cursors`, `history`, `softWrap`, `indent`, `syntaxMap`, `wrapMap`, `snapshot`, `syntaxSnap`, `wrapSnap`, `resolver`, `registry`, `viewport`, `keys`, `styles`, `width`, `height`, `focused`, `offsetX`, `offsetY`, `findOverlay`, `dictation`
- Fields to ADD: `syncFunc`, `sanitizeFunc`, `singleLine`, `readOnly`, `headerHeight`
- Fields to REMOVE: `dirty`, `savedContentHash`, `activeSave`, `filePath`, `highlighter`, `termCaps`, `imageConfig`, `images`, `idAlloc`, `cellSize`, `mouse`, `title`, `breadcrumb`, `lastPlacementSeq`, `pendingPlacementSeq`

**Step 1.2 — Migrate generic commands.**
- Copy commands_registry.go → textedit, register only edit, nav, multi, clipboard, history commands
- Mouse commands: register low-level handlers only (registerMouseCommands returns builder with no registrations — mouse handling is via direct method calls, not command registry)
- Find commands: register stubs (no-ops)
- Commands to exclude: image.*, link.*, file.* (stays with markdownedit/workspace)
- Command name strings are UNCHANGED — `edit.*`, `cursor.*`, `select.*`, `scroll.*`, `clipboard.*`, `history.*`, `multicursor.*` stay as-is (verified: no command uses an `editor.` prefix). Only the Go package path changes from `editor` to `textedit`; renaming the `edit.*` strings would break `app.go` binding verification against the keymap.
- **No LoadFileCmd in textedit (Freeze invariant).** textedit does not own file I/O (D12). The monolith's `LoadFileCmd(path string)` (commands.go:18) is **left untouched** and is deleted with the monolith in Phase 5. The workspace-package `loadFileCmd(ctx context.Context, path string)` is **created new** in `workspace_fileio.go` in Phase 3, and the four callers in `workspace.go` (lines 301, 334, 394, **675**) are switched from `editor.LoadFileCmd(path)` to `loadFileCmd(ctx, path)` in that same Phase 3 commit. There is no intermediate state where the monolith's signature changes.

**Step 1.3 — Migrate generic cell rendering.**
- Copy cell.go → textedit — generic functions only: spanToCells, revealedSpanToCells, renderedSpanToCells, applyOverlays, sliceCells, cellsToString, cellEffectiveStyle, stylesEqual
- NO spanToCellsStyled, NO codeFenceSpanToCells, NO taskListSpanToCells, NO tableSpanToCells, NO headingStyle, NO tableRoleStyle, NO mergeInlineStyle (mergeInlineStyle is used exclusively by tableSpanToCells which lives in markdownedit — it must follow its only caller)

**Step 1.4 — SyncFunc integration.**
- SyncFunc type definition in textedit
- PlainSync in textedit (default)
- Constructor option: `WithSyncFunc(f SyncFunc)`
- `syncDisplay()` (lines 124-145 of apply.go): syntaxMap setup, Sync/SyncNoReveal, wrapMap.Sync, BuildSnapshot, ExpandTableRows. NO ExpandImageRows.
- Constructor option: `WithPaddingTop(n int)` — stored as `m.headerHeight int`

**Step 1.5 — SanitizeFunc + SingleLine + ReadOnly (D9).**
- SanitizeFunc: called in the insert-character command before buffer mutation
- SingleLine: Enter inserts newline → no-op (or emits custom message)
- ReadOnly (D9) — NOT "all mutation returns early". Exactly four things:
  1. `ResolverContext.ReadOnly = m.readOnly` (D4) — `When` clauses gate cut/paste/edit/history automatically; copy/nav/selection/find still resolve.
  2. Guard the **resolver-bypass** mutation paths on `m.readOnly`: printable-char fallback, direct Undo/Redo, direct newline, paste handlers.
  3. View suppresses insertion caret but **keeps selection highlight** (empty `cursorOffsets`, real `selections`).
  4. (markdownedit) handleMouseClick **skips link resolution** — links render but are inert.

**Step 1.6 — NaturalContentHeight method.**
- Walk the WrapSnapshot, count total visual rows at current width
- Returns natural height excluding any header area

**Step 1.7 — textedit.View().**
- Generic cell rendering: spanToCells (no markdown dispatch) → cellsToString
- No image rows, no title, no breadcrumb
- No markdown-specific styling
- Cursor hidden when `m.readOnly`
- Unfocused dim rendering for image placeholder lines (uses imagePlaceholderCells from image_placeholder.go)

**Step 1.8 — textedit FindOverlay.**
- Migrate find_overlay.go — FindOverlay struct with open/close/consumeKey/Visible
- Update() handles FindOpen/FindReplaceOpen keybindings → opens overlay
- When overlay visible, forwards keys to consumeKey()
- Find commands in registry are stubs/no-ops

**Step 1.9 — textedit generic edit primitives (D15) — NOT dictation.**
- Delete editor `dictation.go`. Add generic programmatic-edit primitives to textedit: `ReplaceRange(start, end int, text string) Model`, `AppendText(text string) Model`, `CursorOffset() int`. Built on the existing `applyOperation` pipeline (undoable/history/coalescing).
- Read-only guards ReplaceRange/AppendText (D9); they bump `Revision()` (D13).
- markdownedit wraps ReplaceRange/AppendText (delegate + `afterContentChange()` — D1/D2; see Step 2.x).
- The dictation engine+session is a separate global component (D16, Phase 3) — not here.

### Phase 2: Create markdownedit package (extract from monolith)

**Step 2.1 — Create package scaffold (D1).**
- `pkg/ui/components/markdownedit/markdownedit.go`
- Model embeds `textedit.Model` by value for field/query reuse, but markdownedit owns its **own** `Update` wrapper (D1) — promotion is used only for generic leaf queries, never for the edit/sync/render cycle.
- Fields to ADD: `highlighter`, `termCaps`, `imageConfig`, `images`, `idAlloc`, `cellSize`, `lastPlacementSeq`, `pendingPlacementSeq` (markdown/image fields only)
- Fields NOT added (D12/D13 — workspace-owned): `filePath`, `dirty`, `savedContentHash`, `activeSave`. markdownedit has no file/disk/dirty state.
- `headerHeight` is inherited from textedit, defaults to 0
- Implement the explicit `Update` switch (mouse arms + default delegate-then-`afterContentChange`) and the `afterContentChange()` tail.

**Step 2.2 — MarkdownSync.**
- Move MarkdownSync logic (goldmark parsing, reveal/render, CellMap, coordinate deltas, Chroma highlighting) into markdownedit
- Calls into `pkg/editor/display/` for the heavy lifting
- MarkdownSync becomes the default SyncFunc

**Step 2.3 — Image expansion via `afterContentChange()` (D1, D2).**
- markdownedit does NOT define a `syncDisplay` override (promotion is static — textedit would never call it). Instead `afterContentChange()` reads `textedit.Snapshot()`, applies `ExpandImageRows(snap, m.imageDimsFor)`, and writes it back via `SetSnapshot()`, then re-runs `ScrollToCursor()` + re-clamp.
- imageDimsFor (from image_integration.go) stays in markdownedit — accesses m.images map.

**Step 2.4 — Cell rendering (markdown-specific).**
- Migrate spanToCellsStyled with full markdown dispatch (TokenCodeFence, TokenHeading, TokenBold, TokenItalic, TokenStrikethrough, TokenBlockquote, TokenHorizontalRule, TokenTag, TokenListMarker, TokenTaskList, TokenTable, LinkRoleImage, LinkRoleNavigable)
- Migrate codeFenceSpanToCells (uses m.highlighter — Chroma)
- Migrate taskListSpanToCells, tableSpanToCells, headingStyle, tableRoleStyle, mergeInlineStyle (follows tableSpanToCells, its only caller)

**Step 2.5 — Image pipeline.**
- Migrate image_integration.go, image_allocator.go, render_image.go, render_code.go
- Full image lifecycle (all internal helpers — unexported): discoverNewImages, clearImages, updateImages, emitImagePlacements, buildInlineImagePlacements, retransmitImagesCmd, armImageTicks, detectImageCollapse, resizeImages, imageDimsFor, imagePlaceholderCells, imageMaxCols, handleImagePaste
- imageKittyCapable, imageInlineCapable, imageCapable, imageIDFor → internal (access m.termCaps and m.images — all markdown-specific)
- **Public API mapping for image operations (these are the exported methods callers use):**
  - `RefreshImagesAfterLayoutChange() (Model, tea.Cmd)` → implemented via `retransmitImagesCmd` + `resizeImages` (called by workspace after `SetRect` when terminal reports new cell dimensions)
  - `DeleteAllImagesCmd() tea.Cmd` → implemented via `clearImages` + deletion protocol (called by workspace on quit or tab close to clean up Kitty/iTerm2 image slots)
  - All other image helpers remain unexported internal functions of the markdownedit package

**Step 2.6 — Link resolution (Blocker 1).**
- Migrate resolveLinkClick — accesses m.syntaxSnap (markdownedit-owned)
- handleMouseClick: full handler that:
  1. Checks focused, left button
  2. Computes display point (msg.Y - offsetY - headerHeight)
  3. Skips image-reserved rows
  4. Calls displayToBuffer (from textedit) to get buffer point
  5. If NOT read-only: calls resolveLinkClick with syntaxSnap (read-only → inert links, D9)
  6. If link (and not read-only): emits LinkClickedMsg
  7. Otherwise (no link, or read-only): delegates cursor/selection manipulation to textedit's low-level helpers
- handleMouseMotion: uses textedit's displayToBuffer, then manipulates cursors directly
- handleMouseWheel: scroll + image arm (image-specific)

**Step 2.7 — dispatchOperation, NO save handling (Blocker 9, D1, D2, D12).**
- markdownedit does NOT wrap dispatchOperation via promotion. textedit.dispatchOperation (invoked inside the delegated `textedit.Update`) handles non-save ops + provisional scroll. The markdown tail (image expand, discover, collapse, arm) runs uniformly in `afterContentChange()`.
- **No OperationSaveFile path (D12).** Save is entirely workspace-owned: Cmd+S global → `saveFileCmd(editor.Content())` → `FileSavedMsg` → workspace sets `origContent = activeSave.SavedContent` (D13). The editor has no save method, no SaveIdentity, no activeSave, no `MarkSaved` (it never existed).

**Step 2.8 — markdownedit.View() (Blocker 6).**
- Single-loop rendering over snapshot.Lines (same as current editor/view.go:39-111)
- For each line:
  - If image row: render image placeholder / iTerm2 space cells
  - Else: spanToCellsStyled (markdown dispatch) → cells → sliceCells → applyOverlays → cellsToString
- Join lines, apply MaxWidth/MaxHeight
- Handle unfocused dim rendering (faint text, image lines untouched)
- Title area NOT rendered here — workspace owns title as sibling

**Step 2.9 — Messages (D12).**
- LinkClickedMsg → markdownedit package
- File messages (FileLoadedMsg, FileSavedMsg, etc.) → **workspace package** (D12), NOT textedit. The editor handles no file messages.
- ClipboardContentMsg → textedit (paste = edit concern). **ContentChangedMsg → DELETED (D13)** — dirty is computed by the workspace, gated by `Revision()`.

**Step 2.10 — ImageConfig (Blocker 7).**
- ImageConfig struct stays in markdownedit (was in commands_image.go)
- Defined in markdownedit package

### Phase 3: Rewrite workspace page

**Step 3.1 — Add title + breadcrumb as workspace siblings (D6).**
```go
type Model struct {
    title      title.Model       // dedicated component (relocated), NOT textedit — D6
    breadcrumb breadcrumb.Model  // was editor child, now sibling
    editor     markdownedit.Model
    filePath   string            // D12: workspace owns the path, not the editor
    activeSave SaveIdentity       // D12: in-flight save bookkeeping {RequestID, SavedContent, InFlight}
    lastRev    uint64            // D13: last-seen editor.Revision() to gate dirty recompute
    // ... existing fields (origContent = sole dirty baseline + merge ancestor, etc.)
}
```
New workspace-owned plumbing (D12, D13): `filePath`, `SaveIdentity{RequestID, SavedContent, InFlight}`/`activeSave`, `lastRev`, `isDirty()` (= `Content() != origContent`), and `workspace_fileio.go` (loadFileCmd/saveFileCmd/fileRenameCmd + the file message types). `origContent` is the **sole** baseline (no editor-side dirty — D13).

**Step 3.2 — Update workspace.New().**
- Constructor creates `title.New("Untitled", styles)` for title (dedicated component; rename-debounce/placeholder/revert stay inside it — D6)
- Constructor creates `breadcrumb.New(st, nil)` for breadcrumb — its validator param is ignored (`_`, breadcrumb.go:23), so do NOT pass `validateFilename` here; the workspace calls `validateFilename` directly in its rename path (Edit G / Step 3.5)
- Constructor creates `markdownedit.New(keys, styles, caps)` for main editor (headerHeight defaults to 0)

**Step 3.3 — Update workspace.recalcLayout() (D7, D8).**
- Title gets `SetSize(innerCenterW, 1)` (title.Model is cell-grid only — no offset)
- Content height: `innerH = contentH - 2 - title.Height()` where title.Height() == 1
- markdownedit gets **one** atomic `SetRect(Rect{X: leftW+1, Y: paneTop + 1 + title.Height(), W: innerCenterW, H: innerH})` — `Y` absorbs the top border (1) + title line so absolute image placement and mouse hit-testing land correctly (D7). headerHeight stays 0; the title row lives in `offsetY`, not headerHeight.
- Breadcrumb gets `SetSize(centerW, 1)` (overlaid on bottom border)
- The old separate `m.editor.SetOffset(leftW+1, topOffset)` + `SetSize(...)` pair collapses into the single `SetRect` call (D8).

**Step 3.4 — Update workspace focus routing (D11).**
- paneCenter can be further split into `paneTitle` and `paneContent`
- Up at top of editor → focus title — gated by `m.editor.CursorAtTop()` (D11); the workspace **consumes** the Up keypress on transfer (return early, don't also forward to the editor). This contextual check runs before fallthrough-to-children (§3.4 ordering).
- Enter/Escape in title → focus editor (title's existing `FocusReturnMsg`, D6)
- Mouse click on title area → focus title, forward click position
- Title keys handled in workspace.Update(), not in editor.Update()
- **Any focus change calls `m.dict = m.dict.Disable()`** (D16, policy i) — the dictation session is anchored to a specific buffer and cannot migrate. Fold this into `syncDictationAllowed`/focus transitions.

**Step 3.4b — Dictation component + routing (D16).**
- Add `dict dictation.Model` as a workspace field (new `pkg/ui/components/dictation/`). It owns the engine (ctx/cancel/dictCh + `Ready`/`Listen`/`Partial`/`FinalTranscriptionMsg`, moved out of inline workspace handling) and the session (startOff/appliedLen/enabled).
- `footer.DictationStartMsg` → `m.dict = m.dict.Enable(focusedEditor.CursorOffset())` (anchor to the focused editor); `DictationStopMsg`/focus change → `m.dict.Disable()`.
- Forward engine messages to `m.dict.Update(...)`, then **drain + route synchronously** (§5.4):
```go
m.dict, cmd = m.dict.Update(msg); cmds = append(cmds, cmd)
if s, e, t, ok := m.dict.TakePendingEdit(); ok {
    switch m.focus {
    case paneCenter: m.editor = m.editor.ReplaceRange(s, e, t)
    case paneChat:   m.chat   = m.chat.ApplyToPrompt(s, e, t)
    }
    // dirty recompute (D13) runs via Revision() as after any edit
}
```
- Footer `^v` indicator driven from `m.dict.Enabled()`. `syncDictationAllowed` still gates availability to paneCenter/paneChat.
- The old inline editor dictation handling (`workspace.go:791-847`, `StartDictation`/`ApplyDictationChunk`/`FinalizeDictation`/`CancelDictation` calls + `ContentChangedMsg` emission) is removed.

**Step 3.5 — Update workspace message handling (D12, D13).**
The workspace owns the entire file/disk domain AND dirtiness. The editor never receives a file message and has no dirty state; `origContent` is the sole baseline.
```go
case FileLoadedMsg:   // workspace-package message (D12)
    m.editor   = m.editor.SetContent(string(msg.Content))   // pure content load
    m.filePath = msg.Path                                    // workspace owns the path
    m.breadcrumb = m.breadcrumb.SetPath(msg.Path)
    m.origContent = msg.Content                              // sole baseline + merge ancestor → isDirty()==false
    m.opentabs = m.opentabs.OpenFile(msg.Path)
    m.lastRev = m.editor.Revision()

case FileSavedMsg:    // workspace-package message (D12, D13)
    if m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
        m.activeSave.InFlight = false
        m.origContent = m.activeSave.SavedContent            // BYTES WRITTEN, not Content() — Finding A (data integrity)
        if m.isDirty() {                                     // true iff edited mid-save
            m.opentabs = m.opentabs.MarkDirty(m.filePath)
        } else {
            m.opentabs = m.opentabs.MarkClean(m.filePath)
        }
        // ... pending close/switch follow-up keyed off m.activeSave.RequestID
    }
```
- **Dirty (D13):** `func (m Model) isDirty() bool { return m.editor.Content() != string(m.origContent) }`. Recomputed after editor Update only when `m.editor.Revision() != m.lastRev` (then update `m.lastRev`, mark opentabs, handle untitled-create). No `ContentChangedMsg`.
- **Save (Cmd+S, workspace-global — D4/D12):** `content := m.editor.Content(); m.activeSave = SaveIdentity{RequestID: …, SavedContent: []byte(content), InFlight: true}; cmds = append(cmds, saveFileCmd(SaveRequest{Path: m.filePath, Content: content, RequestID: …}))`. `SaveIdentity`/`activeSave` are workspace fields. Cmd+S is consumed by a direct `key.Matches(msg, m.keys.SaveFile)` arm in `workspace.Update()` (global key, handled before forwarding to the editor — §3.3), so the `file.save` command is never resolved through the registry once the workspace owns save. (The `file.save` CommandBindings entry at `keymap.go:276` is removed in Step 5.2, together with `commands_file.go`.)
- **`workspace.SaveIdentity` is a NEW type defined in the workspace package, NOT a moved copy of `editor.SaveIdentity`.** Field changes vs the monolith (editor.go:40–44 `{Path, RequestID, ContentHash, InFlight}`): `ContentHash string` → `SavedContent []byte` (the bytes actually written — D13 Finding A); `Path` dropped (workspace owns `filePath` as a separate field). The monolith's `editor.SaveIdentity` is deleted with the monolith in Phase 5.
- **Rename:** title's `RenameRequestMsg` → workspace issues `fileRenameCmd(m.filePath, msg.Name)`; on `FileRenamedMsg` → `m.filePath = newPath` + breadcrumb.
- **BreadcrumbView() (workspace.go:724) deleted (D6); BreadcrumbPath()/IsDirty()/MarkSaved() deleted (D12/D13)** — workspace sets breadcrumb from its OWN `m.filePath` and computes dirty from its OWN `m.origContent`.

**Step 3.6 — Update workspace.View().**
- Layout: title → markdownedit → breadcrumb overlay
- Remove `m.editor.BreadcrumbView()` call → use `m.breadcrumb.View()`
- Remove `m.editor.TitleText()` → use `m.title.Text()` (title.Model — D6)
- Title rendered directly: `m.title.View()`
- Workspace composes title + editor vertically. **`SetOffset` is NOT gone (D7) — it is folded into `m.editor.SetRect(...)` whose `Rect.Y` includes the title line + top border.** Do not set markdownedit's Y to the old `topOffset=1`; that would smear images up by `title.Height()` rows.

**Step 3.7 — Title ↔ editor focus transition.**
- **workspace.Update() forwards ALL messages to m.title on every pass** — not just key presses. The dedicated `title.Model` owns the debounce timer + rename state machine (`debounceExpiredMsg` → `RenameRequestMsg`, in `title.go`) — it stays inside the title component (D6), so all `tea.Msg` must reach it. The pattern is:
```go
// Always forward every message — title's own m.focused gates mutation
m.title, cmd = m.title.Update(msg)
cmds = append(cmds, cmd)
```
- Key-press gating: title internally checks `m.focused` (§3.3 pattern) and ignores keys when not focused. workspace does NOT need to pre-filter keys before forwarding.
- When focus == paneCenter and the main editor is focused:
  - On Up, if `m.editor.CursorAtTop()` (D11) → workspace calls `m.editor = m.editor.SetFocused(false)`, `m.title = m.title.SetFocused(true)`, and **returns early (consumes Up)** so the editor doesn't also move the cursor. Otherwise Up forwards to the editor as normal navigation.
- Title emits RenameRequestMsg, FocusReturnMsg → workspace handles them directly (sets focus back to editor, updates breadcrumb)
- In workspace.View(), title area click detection:
  - If click Y < title.Height(), focus title
  - Click is forwarded naturally via the next Update pass (title already received the msg)

### Phase 4: Rewrite chat component

**Step 4.1 — Add markdownedit + textedit fields + focus sub-state (D9, D10).**
```go
type chatFocus int
const ( chatFocusPrompt chatFocus = iota; chatFocusDisplay )

type Model struct {
    display  markdownedit.Model   // read-only conversation view (focusable/selectable/copyable — D9)
    prompt   textedit.Model       // input field with dynamic height
    subFocus chatFocus            // prompt ↔ display, so Cmd+C targets the focused surface (D9)
    // ... keep messages, loading, client, etc. — but NOT scrollOff (deleted, D10)
}
```
- chat routes focus to exactly one of {prompt, display} via `SetFocused`. A key/binding toggles to the display for selection+copy, returns to prompt for input. Mouse click in the display area focuses the display.

**Step 4.2 — Dynamic height logic.**
- On each Update, recompute prompt height:
```
rawH := m.prompt.NaturalContentHeight(innerW)
promptH := clamp(rawH, 3, availH/2)
// prompt (textedit) and display (markdownedit) are offset-bearing → SetRect (D8).
// Y must be the absolute screen row of each sub-view's first content line.
m.prompt  = m.prompt.SetRect(Rect{X: chatX, Y: chatY + availH - promptH, W: innerW, H: promptH})
m.display = m.display.SetRect(Rect{X: chatX, Y: chatY, W: innerW, H: availH - 1 - promptH})
```
The display's `Rect.Y` MUST be the true absolute screen row — it hit-tests the mouse for drag-select (D9).

**Step 4.3 — Message rendering.**
- On each new message, rebuild display buffer from `m.messages[]`
- Set display content via `m.display.SetContent(...)`
- Scroll display to bottom

**Step 4.4 — Input + focus routing (D9).**
- Enter → submits prompt text, clears prompt textedit, sends to AI
- Shift+Enter → inserts newline in prompt textedit
- textedit handles all cursor navigation, selection, backspace, etc.
- **Focus routing (D9):** chat forwards keys to whichever of {prompt, display} is `subFocus`. When `chatFocusDisplay`, keys reach the read-only display (nav/selection/`clipboard.copy` resolve; mutation is gated by `!readOnly`). A toggle binding switches surfaces; a mouse click in the display area sets `chatFocusDisplay`. `SetFocused` is set on exactly one surface per pass.
- **`m.scrollOff` is DELETED (D10).** The display is a real `markdownedit` with its **own** `ViewportState` — conversation scroll IS the display's viewport scroll. A separate chat-level offset would be a second, parallel offset fighting the display's own (exactly the desync the split kills).
  - Keyboard scrollback: focus the display (`chatFocusDisplay`); nav/scroll keys resolve inside it (nav.* is `editorFocused`, works read-only).
  - Mouse wheel over the display area scrolls it **regardless of focus** (wheel msgs carry coordinates → routed to the display by hit-test), so casual scrollback while typing needs no focus switch.
  - No `key.Matches(ScrollUp/ScrollDown/...)` arm in chat.Update() for `m.scrollOff` — it no longer exists.

**Step 4.5 — Remove hand-rolled input AND `m.scrollOff` (D10).**
- Delete `m.input string`, `m.submit()`, and key handling for input text (character insertion, backspace, cursor movement within the prompt field)
- textedit handles all prompt editing
- **Delete `m.scrollOff` and its Up/Down/PgUp/PgDn handlers** (D10) — conversation scroll is now owned by the display markdownedit's viewport (keyboard via display focus, mouse wheel via hit-test). Also delete any hand-rolled conversation-history renderer it drove; the display's `View()` replaces it.

**Step 4.6 — Dictation: delete chat's own path, add `ApplyToPrompt` (D15, D16).**
- Delete chat's `dictationPartial`, `SetDictationPartial`, `FinalizeDictation(text)`, `CancelDictation` — dictation is now the global `dictation.Model` component (D16), not chat-owned.
- Add `func (m Model) ApplyToPrompt(start, end int, text string) Model { m.prompt = m.prompt.ReplaceRange(start, end, text); return m }` — the workspace calls this to route a dictation chunk into the focused prompt (D16). Live-in-buffer (range-replace), matching the main editor; no provisional-greyed display.

### Phase 5: Delete old monolith + update imports

**Step 5.1 — Delete pkg/ui/components/editor/.**
- Remove all files except any that should be kept (e.g. tests that need migration)
- **Relocate** `pkg/ui/components/editor/title/` → `pkg/ui/components/title/` (D6 — the title is NOT deleted; it survives as a dedicated top-level component). Update its import path everywhere (workspace).

**Step 5.2 — Update app.go.**
- Change `editor.RegisterCommands(builder)` to `textedit.RegisterCommands(builder)` + `markdownedit.RegisterCommands(builder)`
- Update imports
- **Delete the `file.save` CommandBindings entry** — remove `add(b.SaveFile, "file.save", "editorFocused")` at `keymap.go:276` from `CommandBindings()`. This MUST happen in the same commit that deletes `commands_file.go` (D12), or `app.go:48` fails startup with `binding references unknown command "file.save"`. The `SaveFile` binding + its help/validation rows (`keymap.go:144`, `:179`) STAY — only the command-name registration is removed; Cmd+S is workspace-global (Step 3.5).

**Step 5.3 — Final test cleanup (D14).**
- The substantive test migration already happened in Phases 1–3 (four-bucket classification — D14). This step is only: delete any leftover `pkg/ui/components/editor/*_test.go`, fix import paths for already-moved tests, confirm no orphaned references to deleted APIs (IsDirty/MarkSaved/StartSave/FilePath/BreadcrumbPath/FileLoadedMsg-on-editor).

**Step 5.4 — Verify compilation and tests.**
- `go build ./...`
- `go test ./pkg/ui/...`
- `go test ./pkg/editor/...`

## TODO Checklist

- [ ] **Phase 1: Create textedit**
  - [ ] 1.1 Package scaffold + Model struct (headerHeight option, NO title/breadcrumb/images)
  - [ ] 1.2 Migrate edit/nav/multi/clipboard/history commands. File I/O Cmds (loadFileCmd/saveFileCmd/fileRenameCmd) do NOT go to textedit — they are created new in the workspace package with `(ctx context.Context, path string)` signatures (D12) in Phase 3, switching all callers in workspace.go (lines 301, 334, 394, 675) at that point. The monolith's `LoadFileCmd` is untouched until Phase 5 (Freeze invariant)
  - [ ] 1.3 Migrate generic cell rendering (spanToCells, cellsToString, etc.) — NO markdown dispatch, NO mergeInlineStyle
  - [ ] 1.4 SyncFunc type + PlainSync + constructor options (WithSyncFunc, WithPaddingTop)
  - [ ] 1.5 SanitizeFunc, SingleLine, ReadOnly (D9: ResolverContext.ReadOnly + bypass-path guards + caret suppression w/ selection kept; markdownedit read-only also makes links inert)
  - [ ] 1.5b Editor has NO dirty state (D13): omit the monolith's dirty machinery (`dirty`/`IsDirty`/`savedContentHash`/`SetDirtyForTest` — note `MarkSaved` never existed) from the new packages; add net-new `Revision() uint64` mutation counter
  - [ ] 1.6 NaturalContentHeight method
  - [ ] 1.7 textedit.View() — generic cell rendering, no images/markdown/title; SetContent clamps TopRow (no baked scroll policy); add AtBottom/GotoBottom/ScrollOffset/SetScrollOffset + Snapshot/SetSnapshot/ScrollToCursor/ContentHeight exported seams (D2, D5); delete dead scrollPreservingAnchor stub
  - [ ] 1.8 FindOverlay migration (stays in textedit, not a command)
  - [ ] 1.9 Delete editor dictation.go; add generic primitives ReplaceRange/AppendText/CursorOffset to textedit (undoable; read-only-guarded D9; Revision-bumping D13; markdownedit-wrapped D1) — D15
  - [ ] 1.10 textedit low-level mouse helpers exported (DisplayToBuffer, MousePositionCursor, MouseExtendSelection, MouseSelectWord, MouseSelectLine, MouseAddCursor) + accessors (SyntaxSnap/WrapSnap/Viewport) (D3); keep textedit's own MouseClickMsg arm for standalone instances; register find.* stubs + mouse no-ops and wire ResolverContext.ReadOnly (D4)
  - [ ] 1.11 textedit tests compile + pass — migrate bucket-1 tests (content/edit/nav/cursor/history/clipboard/generic-cell) into `package textedit`; duplicate generic helpers (setCursor/runCmd/firstMsg/newTestEditor) into textedit testhelpers (D14)

- [ ] **Phase 2: Create markdownedit**
  - [ ] 2.1 Package scaffold + Model struct (embeds textedit.Model for reuse; owns its OWN Update wrapper + afterContentChange tail — NO promotion-override of the edit/sync/render cycle — D1)
  - [ ] 2.2 MarkdownSync (goldmark + Chroma via pkg/editor/display/)
  - [ ] 2.3 afterContentChange() tail: Snapshot→ExpandImageRows→SetSnapshot→ScrollToCursor→re-clamp→discover→collapse→arm (D1, D2) — NOT a syncDisplay promotion-override
  - [ ] 2.4 Markdown cell rendering (spanToCellsStyled, codeFenceSpanToCells, mergeInlineStyle, etc.)
  - [ ] 2.5 Image pipeline (discover, load, place, animate, cleanup); verify RefreshImagesAfterLayoutChange → retransmitImagesCmd+resizeImages and DeleteAllImagesCmd → clearImages+delete protocol are wired
  - [ ] 2.6 Link resolution (resolveLinkClick with syntaxSnap — Blocker 1)
  - [ ] 2.7 handleMouseClick with link-aware dispatch (delegates to textedit for non-link)
  - [ ] 2.8 dispatchOperation — NO OperationSaveFile (D12). textedit handles OperationHistory/Scroll/None + provisional scroll; markdown tail (image expand/discover/collapse/arm) runs in afterContentChange() only. Save is workspace-owned (Cmd+S → workspace saveFileCmd), not in dispatchOperation.
  - [ ] 2.9 markdownedit.View() — single-loop image+text with markdown styling (Blocker 6)
  - [ ] 2.12 emitImagePlacements as OUTERMOST Update wrapper; placementTickMsg arm in routeUpdate; verify offsetY/headerHeight separation (D7)
  - [ ] 2.13 SetRect(Rect) replaces SetSize+SetOffset on markdownedit (D8)
  - [ ] 2.10 ImageConfig in markdownedit package (Blocker 7)
  - [ ] 2.11 markdownedit tests compile + pass — migrate bucket-2 tests (markdown/image/link/highlight/styled-cell) into `package markdownedit`; markdownedit testhelpers (writePNG/kittyCaps + docEditor rewritten to SetContent(content)+SetDir, D14)

- [ ] **Phase 3: Rewrite workspace**
  - [ ] 3.1 Add title (dedicated title.Model — D6) + breadcrumb as workspace siblings
  - [ ] 3.2 Update workspace.New() constructors
  - [ ] 3.3 Update recalcLayout() — title height, content height (Blocker 4); markdownedit `SetRect` with `Rect.Y = paneTop + border + title.Height()` (D7, D8)
  - [ ] 3.4 Update focus routing (title ↔ markdownedit) via `m.editor.CursorAtTop()`; workspace consumes Up on transfer (D11); ANY focus change → `m.dict.Disable()` (D16)
  - [ ] 3.4b Dictation component (D16): new `pkg/ui/components/dictation/` owning engine+session+enabled; workspace routes `TakePendingEdit()` → `ReplaceRange` on focused editor (paneCenter) or `chat.ApplyToPrompt` (paneChat); Enable anchors to `focusedEditor.CursorOffset()`; remove inline editor-dictation handling (workspace.go:791-847) + ContentChangedMsg emission; footer `^v` from `m.dict.Enabled()`
  - [ ] 3.5 Workspace owns file/disk domain + dirtiness (D12, D13): FileLoadedMsg→SetContent+own filePath+breadcrumb+origContent; FileSavedMsg→`origContent = activeSave.SavedContent` (BYTES WRITTEN — Finding A data-integrity fix, NOT Content()); dirty = `Content() != origContent` gated by `Revision()`/`lastRev`; Cmd+S global save via workspace saveFileCmd; rename via workspace; workspace owns SaveIdentity (new type `{RequestID, SavedContent, InFlight}`)/activeSave + file messages + I/O Cmds in workspace_fileio.go; DELETE ContentChangedMsg; the editor's BreadcrumbView/BreadcrumbPath/IsDirty/dirty/savedContentHash die with the monolith in Phase 5 (no MarkSaved — never existed)
  - [ ] 3.6 Update workspace.View() — title + editor composition (m.breadcrumb.View() replaces m.editor.BreadcrumbView())
  - [ ] 3.7 Title ↔ editor focus transition — workspace forwards ALL messages to m.title every Update pass (debounce timers require it); key-press gating is internal to title
  - [ ] 3.8 workspace tests pass — REWRITE bucket-3 save/dirty tests as `package workspace` tests (D14): TestStaleSavesIgnored, TestDuplicateOutOfOrderSaves, file-load half of TestEditorIntegration, rename, filename_validation. Assert D13 invariants (stale ack doesn't clobber origContent/dirty; matching ack sets origContent = activeSave.SavedContent). Make-dirty-by-editing, not by flag-poke. These ship WITH the D12/D13 code, not in Phase 5.

- [ ] **Phase 4: Rewrite chat**
  - [ ] 4.1 Add display markdownedit + prompt textedit fields + prompt↔display focus sub-state (D9); display read-only is focusable/selectable/copyable with inert links
  - [ ] 4.2 Dynamic height logic (NaturalContentHeight + clamp)
  - [ ] 4.3 Message → buffer rebuild on new response
  - [ ] 4.4 Enter submit, Shift+Enter newline; prompt↔display focus routing (D9); DELETE m.scrollOff scroll handlers — display owns conversation scroll (keyboard via display focus, wheel via hit-test) (D10)
  - [ ] 4.5 Remove hand-rolled input field + append/backspace logic AND m.scrollOff + its handlers + hand-rolled history renderer (D10)
  - [ ] 4.6 Dictation (D15/D16): delete chat's dictationPartial/SetDictationPartial/FinalizeDictation(text)/CancelDictation; add `ApplyToPrompt(start,end,text)` → `prompt.ReplaceRange`
  - [ ] 4.7 chat tests pass

- [ ] **Phase 5: Cleanup**
  - [ ] 5.1 Delete pkg/ui/components/editor/; RELOCATE title/ → pkg/ui/components/title/ (NOT deleted — D6)
  - [ ] 5.2 Update app.go (register commands from new packages — textedit + markdownedit; NO file.save command, D12); remove keymap.go:276 file.save CommandBindings entry (else app.go:48 startup error)
  - [ ] 5.3 Final test cleanup (D14): delete leftover editor/*_test.go; fix import paths; confirm no orphaned refs to deleted APIs. (Substantive migration already done in Phases 1–3.)
  - [ ] 5.4 Update CLAUDE.md §10 (add textedit/ + markdownedit/ + title/ + dictation/ at top level) and §2.2 (SetRect(Rect) for offset-bearing components vs SetSize(w,h) for cell-grid-only — D8; note non-rendering controller components like dictation, D16)
  - [ ] 5.5 `go build ./...` + `go test ./pkg/...`

## ADR Candidates

- [ ] ADR-0004: Workspace owns the dedicated `title.Model` (not textedit, not markdownedit) and breadcrumb — D6
- [ ] ADR-0005: markdownedit owns full rendering loop (not textedit.View() sub-composition)
- [ ] ADR-0006: Mouse link resolution lives in markdownedit (textedit provides low-level helpers) — D3
- [ ] ADR-0007: Cell rendering split — textedit has generic, markdownedit has markdown-specific
- [ ] ADR-0008: FindOverlay stays in textedit as UI state (not a command)
- [ ] ADR-0009: markdownedit composes textedit via explicit `Update` wrapper + `afterContentChange()` tail, NOT method-promotion override (Go promotion is static dispatch) — D1, D2
- [ ] ADR-0010: Command registry is a single shared union injected everywhere; ReadOnly flows into ResolverContext; save is a workspace-global — D4
- [ ] ADR-0011: Scroll policy is caller-chosen; SetContent bakes none; §5.7 re-expressed for custom ViewportState — D5
- [ ] ADR-0012: Inline images paint at absolute terminal coords; markdownedit.offsetY absorbs the external title; emitImagePlacements is the outermost Update wrapper — D7
- [ ] ADR-0013: Scoped `SetRect(Rect)` (atomic position+size) on offset-bearing components; cell-grid-only components keep `SetSize(w,h)` — D8
- [ ] ADR-0014: ReadOnly = declarative `When` gating + bypass-path guards + caret suppression + inert links; read-only ≠ non-interactive (chat display is focusable/selectable/copyable) — D9
- [ ] ADR-0015: Delete chat `m.scrollOff`; conversation scroll owned by the display markdownedit's viewport — D10
- [ ] ADR-0016: Title↔content focus transition via `CursorAtTop()` query (no workspace access to editor internals); workspace consumes the Up keypress — D11
- [ ] ADR-0017: Editor knows only content+edits (`SetContent(string)`/`Content`/`Revision`); entire file/disk domain (path, I/O Cmds, file messages, save orchestration) lives in the workspace package — D12
- [ ] ADR-0018: Dirtiness is workspace-owned — `origContent` is the sole baseline, dirty = `Content() != origContent`; no editor dirty state; save-ack baseline = bytes written (data-integrity fix); ContentChangedMsg deleted — D13
- [ ] ADR-0019: Test migration is a 4-bucket reclassification; save/dirty tests rewritten as workspace tests in Phase 3 (ship with the D13 data-integrity fix); generic helpers duplicated per package — D14
- [ ] ADR-0020: Dictation is not an editor feature — textedit exposes generic `ReplaceRange`/`AppendText` primitives (for any programmatic text producer); editor `dictation.go` deleted — D15
- [ ] ADR-0021: Dictation is a global workspace-owned component (engine+session+enabled); workspace routes chunks into the focused editor via `ReplaceRange` (value semantics); focus change ends the session — D16

## Blocker Resolution Summary

> **Note:** this table predates the grill-me decisions (D1–D16) at the top of the doc, which are authoritative where they conflict. Rows 3, 4, 5, 9 are annotated below where a decision superseded the original resolution.

| # | Blocker | Severity | Resolution |
|---|---------|----------|------------|
| 1 | Mouse link click depends on syntaxSnap | High | Link resolution stays in markdownedit. textedit provides displayToBuffer and low-level mouse helpers. handleMouseClick in markdownedit bridges both. |
| 2 | spanToCellsStyled has markdown dispatch | High | spanToCellsStyled moves to markdownedit. textedit gets generic spanToCells only. |
| 3 | syncDisplay calls imageDimsFor | High | textedit.syncDisplay does syntax+wrap+snapshot+tableExpand. **D1/D2 supersede:** markdownedit does NOT wrap `syncDisplay` (static dispatch); it re-expands images in `afterContentChange()` after delegating to `textedit.Update`. |
| 4 | contentHeight depends on title | High | `WithPaddingTop(n)`/headerHeight=0. **D7/D8 supersede the sizing path:** workspace positions markdownedit via `SetRect` whose `Rect.Y` absorbs the title; content height passed in the same `Rect`. |
| 5 | headerHeight in mouse/image calculations | Medium | headerHeight=0 on markdownedit. **D7 supersedes:** the title row lives in `offsetY` (via `SetRect.Y`), not headerHeight; positioning is still mandatory (the old "workspace need not set this" was wrong). |
| 6 | View interleaves images in single loop | Medium | markdownedit.View keeps single-loop approach (does NOT call textedit.View). textedit.View is a separate simpler implementation for title/chat-prompt. |
| 7 | ImageConfig type location | Medium | ImageConfig stays in markdownedit package (was in editor package). |
| 8 | FindOverlay ownership | Medium | FindOverlay stays in textedit as internal UI state. Find commands are stubs. Overlay opened/closed via direct methods. |
| 9 | dispatchOperation handles save | Medium | textedit.dispatchOperation handles non-save operations. **D12 supersedes:** there is NO save in dispatchOperation at all — the `file.save` command + `OperationSaveFile` are removed; save is a workspace-global (Cmd+S → workspace `saveFileCmd` → `FileSavedMsg` → `origContent` update). |
| 10 | codeFenceSpanToCells uses highlighter | Low | codeFenceSpanToCells stays in markdownedit (has m.highlighter). textedit has no code fence handling. |

## Critic Review Resolution

The following table maps each critic finding to its resolution in this plan:

| # | Severity | Critic Issue | Resolution |
|---|----------|-------------|------------|
| 1 | Major | recalcLayout() called from Init() with zero dimensions | **Fixed.** Component Contracts section explicitly states recalcLayout() is an internal helper called only from Update() on tea.WindowSizeMsg or structural changes. NOT from Init(). |
| 2 | Major | tea.WindowSizeMsg should NOT be handled by child components | **Fixed.** Component Contracts section explicitly states: "Child components (textedit, markdownedit) do NOT handle tea.WindowSizeMsg. Only the workspace page receives window resize and distributes via SetSize." |
| 3 | Major | SetWidth/SetHeight instead of single SetSize(w, h) | **Fixed.** textedit and markdownedit both expose `SetSize(w, h int) Model` — a single method for both dimensions. No separate SetWidth or SetHeight. |
| 4 | Major | Missing consumer-side message handling (orphaned messages) | **Fixed** (routing under D12: workspace handles FileSelectedMsg → its own `loadFileCmd` → FileLoadedMsg → `editor.SetContent` + breadcrumb + opentabs; file Cmds/messages are workspace-owned, not editor.LoadFileCmd). |
| 5 | Major | Missing scroll preservation pattern in editor | **Fixed.** Scroll Preservation section documents the AtBottom/YOffset save-restore pattern. SetContent method notes scroll preservation. Both textedit and markdownedit follow this pattern. |
| 6 | Major | Missing SetFocused in component specs | **Fixed.** SetFocused(bool) Model is listed in the Component Contracts section and in the carried methods for textedit. All components that manage focus expose SetFocused. |
| 7 | Major | Missing context cancellation in LoadFileCmd | **Fixed.** commands.go file mapping shows `LoadFileCmd(ctx context.Context, path string) tea.Cmd`. Message Ownership section documents context parameter. Rationale: without cancellation, slow file load can't be interrupted by navigation. |
| 8 | Moderate | chordHint/chordState residency unclear | **Resolved.** The plan does NOT include chordHint or chordState in textedit or markdownedit. Chord state (confirm-exit) belongs exclusively to footer per §3.2 of CLAUDE.md. No chord state exists in editor components. |
| 9 | Moderate | filepane → workspace message forwarding not shown | **Resolved.** The plan does not use "filepane" — the filetree communicates directly with workspace. workspace routes filetree.FileSelectedMsg to its own `loadFileCmd` (workspace-owned per D12). No intermediate pass-through component needed. |
| 10 | Moderate | Cmd batching pattern inconsistent | **Fixed.** Cmd Batching Convention section mandates: all Update() methods use `var cmds []tea.Cmd; ...; return m, tea.Batch(cmds...)`. No arm returns m, nil or m, cmd directly. |
| 11 | Minor | recalcLayout() not listed as method, called with zero dims in Init() | **Fixed.** recalcLayout() is documented as an internal helper (not public contract) in Component Contracts. Explicitly excluded from Init(). |
| 12 | Minor | breadcrumb field not declared in editor spec | **Resolved.** breadcrumb was explicitly removed from markdownedit (Step 1.1: "Fields to REMOVE: ... breadcrumb"). Breadcrumb is a workspace sibling, not an editor field. markdownedit.View() does NOT render m.breadcrumb. |
| 13 | Minor | workspace View() doesn't show pane dividers | **Acknowledged.** workspace.View() renders dividers between panes (filetree \| editor) using lipgloss border styles per §4.3. This is implementation detail in the page, not a component contract concern. |
| 14 | Minor | New filepane directory not acknowledged in §10 structure | **Resolved.** The plan does not introduce "filepane" — it introduces `textedit` and `markdownedit` under `pkg/ui/components/`. The §10 directory structure in CLAUDE.md should be updated to add these two packages. |

## Second Critic Round Resolution

> **Note:** this table predates the grill-me decisions (D1–D16). Row 7 (chat `m.scrollOff`) is **superseded by D10** — see the annotation in that row; `m.scrollOff` is deleted, not kept.

| # | Severity | Critic Issue | Resolution |
|---|----------|-------------|------------|
| 1 | High | LoadFileCmd signature mismatch — monolith has `(path string)`, plan requires `(ctx, path)` | **SUPERSEDED by the Build-green/Freeze invariant (under `## Phases`).** The monolith's `LoadFileCmd(path)` is NOT mutated — it is left intact and deleted in Phase 5. The workspace-package `loadFileCmd(ctx, path)` is created new in Phase 3 (`workspace_fileio.go`), switching callers in workspace.go (lines 301, 334, 394, **675**) at that point. The original "apply atomically" resolution is void. Canonical source: Freeze invariant + Step 1.2 + Edit B. |
| 2 | Medium | Scroll preservation presented as "carried" but not implemented in monolith | **Fixed.** Scroll Preservation section now begins with "Status: NOT yet implemented in the monolith" and cites the dead `scrollPreservingAnchor` stub. Steps 1.7 and 2.9 explicitly say "must be built, not carried". |
| 3 | Medium | BreadcrumbView() call site (workspace.go:724) not covered after editor split | **Fixed, then SUPERSEDED by D12.** BreadcrumbView() is deleted. ~~workspace calls `m.breadcrumb.SetPath(m.editor.BreadcrumbPath())`~~ — under D12 `BreadcrumbPath()` is also deleted; the workspace owns `filePath` and sets the breadcrumb from its OWN path on FileLoadedMsg/FileRenamedMsg. See Step 3.5. |
| 4 | Medium | Phase 3.7 only described key routing for title — timers require ALL messages | **Fixed.** Step 3.7 now specifies that workspace.Update() forwards ALL messages to m.title on every pass via `m.title, cmd = m.title.Update(msg)`. Key-press gating is internal to title per §3.3. Checklist updated. |
| 5 | Low | mergeInlineStyle listed in textedit but used exclusively by tableSpanToCells (markdownedit) — illegal lateral dependency | **Fixed.** mergeInlineStyle removed from textedit in: Cell rendering split description, Step 1.3, and textedit file mapping table. Added to markdownedit in: Cell rendering split, Step 2.4, and markdownedit file mapping table (was already there — the textedit claim was the contradiction). |
| 6 | Medium | Image pipeline public API not enumerated — RefreshImagesAfterLayoutChange and DeleteAllImagesCmd not mapped to source functions | **Fixed.** Step 2.5 now explicitly maps: RefreshImagesAfterLayoutChange → retransmitImagesCmd + resizeImages, DeleteAllImagesCmd → clearImages + deletion protocol. All other image helpers listed as unexported internal. |
| 7 | Low | Chat scrollOff key handlers (Up/Down/PgUp/PgDn on conversation history) not preserved after prompt becomes textedit | **SUPERSEDED by D10.** `m.scrollOff` IS deleted; conversation scroll is owned by the display `markdownedit`'s own `ViewportState` (keyboard via display focus, mouse wheel via hit-test). The original "keep m.scrollOff and its key handlers" resolution is void. Canonical source: D10 + Steps 4.4/4.5. |

## Grill-Me Round Resolution (verified against the monolith)

| D | Decision | Why the old plan was wrong | Where applied |
|---|----------|----------------------------|---------------|
| D1 | markdownedit owns its own `Update` wrapper; promotion only for generic leaf queries | "Embeds textedit.Model (promotes all textedit methods)" + "markdownedit.syncDisplay wraps textedit.syncDisplay" assumed virtual dispatch. Go promotion is **static** — textedit's internal `syncDisplay`/`dispatchOperation` calls never reach markdownedit's versions, so image expansion + markdown styling would silently never run after an edit. | Architecture Decisions; markdownedit API; Sync Pipeline; Steps 2.1, 2.3, 2.7 |
| D2 | Seam: textedit provisional, markdownedit final via `afterContentChange()` within the same pass; re-scroll absolute, re-clamp without delta | `scrollToCursor` runs inside textedit on the image-free snapshot; `ExpandImageRows` inserts rows afterward, invalidating the computed offset. Needs a deterministic re-run on the expanded snapshot. textedit must export `Snapshot/SetSnapshot/ScrollToCursor/ContentHeight`. | Architecture Decisions; Sync Pipeline; markdownedit API |
| D3 | markdownedit intercepts mouse messages; calls textedit's exported low-level helpers (never `textedit.Update`); textedit keeps its own click arm | Mouse clicks are handled directly in `editor.Update` (`editor.go:239-246`), not via the registry. Blind delegation would move the cursor on a link click before link detection. Requires a wide exported mouse API. | Architecture Decisions; Mouse Handling; markdownedit Update; Steps 1.10, 2.1 |
| D4 | Single shared-union registry injected everywhere; ReadOnly→ResolverContext; save is workspace-global | One global registry + shared resolver + `app.go:44-58` startup verification make per-component registries impossible. Monolith hardcodes `ReadOnly:false` (`editor.go:398`). find.*/mouse must be registered as stubs to pass verification. | Architecture Decisions; textedit commands; Step 1.10 |
| D5 | Caller-chosen scroll policy; SetContent clamps but bakes none; §5.7 rewritten for `ViewportState` | Editor uses custom `ViewportState{TopRow,ScrollCol}` (`editor.go:30-33`) — the §5.7 snippet calls bubbles-viewport methods (`AtBottom/YOffset/GotoBottom`) that don't exist and won't compile. FileLoadedMsg should reset, not preserve/bottom-stick. | Scroll Preservation; textedit API; Steps 1.7 |
| D6 | Title stays a dedicated `title.Model` (relocated to `pkg/ui/components/title/`); only the chat prompt becomes textedit | title-as-textedit would scatter rename-debounce/placeholder/revert (`title.go`) into the workspace and require gating find/dictation/multicursor/softwrap/syntax — all nonsensical for a filename. | Architecture Decisions; Ownership; Instance Config; Steps 3.1, 3.2, 3.6, 3.7, 5.1 |
| D7 | markdownedit.offsetY absorbs the external title (Rect.Y = paneTop+border+titleHeight, headerHeight=0); emitImagePlacements is the outermost Update wrapper | Images paint at ABSOLUTE coords via `tea.Raw` `\033[row;colH` using `offsetY + headerHeight` (`image_integration.go:54,79`). Old plan said "workspace does NOT need to set this" / "No more SetOffset" — wrong: offsetY must shift by titleHeight or images smear & clicks mis-hit. emitImagePlacements is outermost in monolith (`editor.go:136-144`); mouse arms change viewport→placement seq. | Architecture Decisions; Header Height; markdownedit Update; Steps 3.3, 3.6, 2.12 |
| D8 | Scoped `SetRect(Rect)` on offset-bearing components (textedit, markdownedit, filetree); cell-grid-only (footer, breadcrumb, opentabs, chat) keep `SetSize(w,h)` | Survey: only 2 of 6 components have offset (editor, filetree); offset is a leaky-abstraction concession (absolute paint + mouse), not general geometry. Universal SetRectangle would force a position concept onto pure cell-renderers and diverge from CLAUDE.md §2.2. Atomic SetRect prevents the D7 desync where it matters. | Architecture Decisions; Component Contracts; API surface; Steps 3.3, 5.4, 2.13 |
| D9 | ReadOnly = ResolverContext.ReadOnly + bypass-path guards + caret suppression (selection kept) + inert links; read-only ≠ non-interactive | Read-only gating is already declarative in `When` clauses (`commands_clipboard.go:37,48,59` etc.) evaluated by the resolver (`parser.go:25-26`). "All mutation returns early" is redundant and would wrongly kill `clipboard.copy` (which is `editorFocused`, not `!readOnly`). Chat display needs selection+copy → focusable, truthful Rect.Y, prompt↔display focus sub-state. Link clicks stay main-editor-only (user scope cut). | Architecture Decisions; Step 1.5; mouse handlers; chat instance config; Steps 4.1, 4.4 |
| D10 | Delete chat `m.scrollOff`; display markdownedit owns conversation scroll via its viewport | Once the conversation is a real markdownedit (D9), a chat-level offset duplicates and fights the display's own `ViewportState`. Keyboard scrollback = focus display + nav (works read-only); wheel scrolls via hit-test regardless of focus. | Architecture Decisions; Steps 4.1, 4.4, 4.5 |
| D11 | Title↔content focus transition via `CursorAtTop()` query on textedit (promoted); workspace owns transition, consumes Up | Monolith check (`editor.go:344-358`) needs cursors+buf+syntaxSnap+wrapSnap+viewport — workspace can't replicate without importing display/coords internals (§2.1, §10). One query keeps the geometry inside the component. Workspace must consume Up or the cursor also moves / both panes act. | Architecture Decisions; exported seams; Steps 3.4, 3.7 |
| D12 | Editor = content+edits only; entire file/disk domain → workspace package (option A, no `document` pkg) | File logic was split awkwardly (editor: filePath/savedContentHash/activeSave/StartSave + 9 file-msg handlers; workspace: origContent/merge/watch/createFileCmd). `SetContent(string)` is the right shape. SetDir stays (image-resolution = rendering). file.save command + OperationSaveFile removed (save is workspace-global, finally coherent with D4). Simplifies D4/D6 (BreadcrumbPath/StartSave deleted). | Architecture Decisions; Message table; API surface; file mapping (commands.go/commands_file.go/messages.go/editor.go); Steps 1.2, 2.7, 2.9, 3.1, 3.5; dispatchOperation |
| D13 | Dirtiness is workspace-owned (amends D12): `origContent` is sole baseline, dirty = `Content() != origContent`; editor has NO dirty state; save-ack baseline = bytes written; ContentChangedMsg deleted; editor exposes `Revision()` | Two baselines (editor savedContentHash + workspace origContent) risk drift; collapsing to one removes it. Monolith already sets ancestor = `msg.SavedContent` (workspace.go:690) but gates the ack on path (`msg.Path == m.editor.FilePath()`), so a late ack from a superseded same-file save still clobbers `origContent` mid-flight → merge-ancestor corruption / data loss (§1.3). Fix: gate on `activeSave.RequestID`. ContentChangedMsg carried a now-meaningless Path + was sync ping-pong (§5.4). `Revision()` gates O(n) dirty recompute. | Architecture Decisions; D12 KEEPS list; API surface; editor.go mapping; messages split; Steps 1.5b, 3.1, 3.5 |
| D14 | Test migration = 4-bucket reclassification (textedit/markdownedit/workspace); save/dirty tests REWRITTEN as workspace tests in Phase 3; generic helpers duplicated | `editor_test.go` has 21 file/save/dirty refs poking StartSave/activeSave/dirty/filePath — un-compilable post-split (D12/D13); the two stale/out-of-order save tests are the D13 data-loss regression guard and must ship with the fix. Step-5.3 "fix references" understated ~5,100 LoC of work. Helpers can't cross package boundaries → duplicate. | Architecture Decisions; Steps 1.11, 2.11, 3.8, 5.3 |
| D15 | Dictation is not an editor feature; textedit exposes generic `ReplaceRange`/`AppendText`/`CursorOffset`; editor `dictation.go` deleted | Same principle as D12/D13 one level deeper — editor owns content + generic mutation API, features compose on top. Monolith baked `dictationState` + 5 dictation methods into the editor. Generic primitives serve dictation AND future producers (LLM completion, snippets). markdownedit-wrapped (D1), read-only-guarded (D9), Revision-bumping (D13). | Architecture Decisions; textedit API; file mapping (dictation.go); Steps 1.9, 4.6 |
| D16 | Dictation is a global workspace-owned component; workspace routes chunks into focused editor; focus change ends session | "Sends into focused editor" can't be literal (§1.1 value semantics — no sibling refs). Split: `dictation.Model` owns engine+session+enabled; workspace owns routing (`TakePendingEdit` → `ReplaceRange` on focused child, §5.4 synchronous drain). startOff is per-buffer → focus change auto-Disables (policy i). chat loses its own dictation path (one session, one component). | Architecture Decisions; ownership tree; Steps 3.4, 3.4b, 4.6; §10 |
