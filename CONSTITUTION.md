# The Rune Constitution

This is how we do things here. When designing a new feature or reviewing a change, ensure every relevant article holds — articles are binding, not advisory.

Article numbers (§) are **frozen**: source comments and tests cite them (e.g. §1.4.8, §5.4). Amend an article in place via PR; never renumber or delete one. First time in the repo? `CLAUDE.md` orients you; this file is the law.

---

## §0 Prime Directive — Protect the User's Words

Never corrupt and never lose what the user wrote. When data safety conflicts with performance, elegance, or features — data safety wins.

### §0.1 The Harm Ladder

Rank every defect by the highest rung it can reach; always trade a failure DOWN the ladder:

1. **Catastrophic — silent corruption.** Wrong/garbled/reordered bytes, mangled UTF-8, a silent rewrite (line endings, trailing newline, BOM, encoding), a good file overwritten by a bad buffer. Never ships.
2. **Severe — losing more than a few seconds of work.** A crash, failed save, or botched recovery that discards unsaved edits.
3. **Tolerable — everything else.** Render glitch, wrong layout, dropped keypress, a clean crash that loses nothing.

Prefer a Tolerable halt — a surfaced error that keeps the buffer — over any higher rung. Halt with a visible error, never `panic` (a panic takes the unsaved buffer with it).

---

## §1 Go Fundamentals

### §1.1 Value Semantics
- Prefer values. Take a pointer only for a struct holding a `sync.Mutex` or owning a unique OS resource.
- Own child models **by value**.
- Make zero values meaningful: `type Result struct { Data T; Valid bool }` over `*T` + nil checks.
- Reset a reusable slice with `xs = xs[:0]` (keeps the backing array); use `= nil` only to release memory you won't refill.

### §1.2 Concurrency
Cross goroutine boundaries by copy or immutable value. In a `tea.Cmd` closure, read only locals captured before it:
```go
idx := m.opentabs.Cursor()                   // ✓ capture, then close over
return func() tea.Msg { return PinMsg{idx} } // reading m inside runs later → race
```

### §1.3 Error Handling
- **Fail fast on data risk.** On any suspicion user data is lost or corrupted, stop and surface a hard error.
- **Clamp every edit range** to the live byte length on UTF-8 boundaries; surface or log a rejected clamp. All edits funnel through `buffer.ApplyEdits` / `textedit.ReplaceRange`; offsets are BYTES (§1.5).
  ```go
  end = min(end, len(content)) // ✓ a stale range is a corruption vector
  ```
- **Treat a destructive async edit as suspect-until-proven.** An edit that empties a non-empty region or shrinks the doc far below load size (dictation, paste, IME, watcher) is clamped or dropped before it reaches the buffer — an empty interim reset is not a user deletion.
- **Handle every returned error**; to ignore one, annotate it: `_ = f() // fire-and-forget: <reason>`.
- **Wrap with context and `%w`**: `fmt.Errorf("load dir %q: %w", dir, err)`. Keep errors as `error`; call `.Error()` only at the display boundary.
- **Surface invalid input** — no silent fallback or default.
- **Halt, never `panic`.** Enforce an invariant with graceful degradation or a test-only assert (`//go:build testing`). A rendering error is Tolerable; data loss is not.

### §1.4 Data Persistence & Durability

Implementation-agnostic laws; symbols in parentheses are today's grep-resolvable wiring.

- **§1.4.1 Atomic durable writes.** Write user content only through `atomicfile.Write` (temp in target dir → fsync → rename → fsync parent). The buffer is clean only after the rename returns.
- **§1.4.2 Unsaved work goes to the recovery store** (`docstate.AppendEdit` / `CreateSnapshot`), never the user's `.md`. The destination changes only on an explicit act — ⌘S or save-on-close — through `Materialize`.
- **§1.4.3 Recovery state is durable** — it must survive the crash it exists for. On open, reconstruct unsaved changes as *editable* history (`RecoverDocument`); never silently write recovered content over the file.
- **§1.4.4 Guard every destructive transition.** Close/quit/switch/overwrite on a dirty buffer prompts (save/discard/cancel) or preserves. The confirmation state lives in the component that renders it (§3.2).
- **§1.4.5 Byte-faithful round-trip.** Load → edit → save is byte-identical except where the user edited: line endings, trailing newline, BOM, and encoding pass through verbatim. Normalization is an explicit, opt-in user action.
- **§1.4.6 Key history to file identity** — inode+device (`vfs` file ID), path as fallback only; tell the user on a detected rename.
- **§1.4.7 An externally-changed file is a hazard.** Before overwriting, detect divergence from what was loaded and refuse-or-prompt (`Materialize`'s CAS expectation; `Probe` classifies `Clean | BufferAhead | DiskAhead | Diverged`).
- **§1.4.8 Derive dirty on every transition** (open, switch, evict, close, quit) from the durable journal position vs the last-saved position (`Sync` / `IsDirty` / `DirtyDocs`) — recompute from the store each time; any cached flag is render-only.
- **§1.4.9 Reach the filesystem only through the injected `vfs.FS`.** `cmd/rune` constructs exactly ONE `vfs.FS` and injects it via `workspace.WithFS`, which propagates it to `docstate.UseFS` and the editor's `SetFS`; a `nil` shim defaults to `vfs.Disk{}` only where nothing was injected. Real `os.*` lives at two boundaries — `pkg/vfs` (`Disk` wraps os, `Mem` is in-memory) and `pkg/atomicfile` — plus two documented exceptions: launch bootstrap (`os.Getwd`, `-w` validation, the SQLite DB path), and test/fuzz tooling. The fsnotify directory watcher is a separate OS resource (not file *content* I/O, so outside `vfs.FS` itself) with its own mirrored seam — `workspace.Watcher`, injected via `WithWatcher`; a `nil` watcher defaults to `FSNotifyWatcher`, and every test constructor plus the session fuzzer inject `NoopWatcher` so no real fsnotify goroutine is ever spawned outside production.
- **§1.4.10 Capture before discard — physically.** Bytes a write path displaces are captured as a durable blob BEFORE they're gone, guaranteed by mechanism, not by careful ordering: `Materialize` swaps via `vfs.Exchange` (atomic), re-reads and re-hashes what the swap displaced, and on any mismatch with the CAS expectation commits those bytes as a blob before the temp file is removed. No window exists for a race to land in. The in-memory analogue is §1.3's clamp-or-drop rule.

### §1.5 Two Coordinate Systems — Bytes vs Runes
- **Buffer / edit / cursor offsets are BYTES.** Compute with `len(...)`; never split a UTF-8 rune.
- **Display width is RUNES.** Column, wrap boundary, cell width use `utf8.RuneCountInString`.
```go
end := start + len(insertedText)                 // edit offset — BYTES ✓
col := utf8.RuneCountInString(line[:cursorByte]) // display column — RUNES ✓
```
If a number indexes the buffer it's bytes; if it positions something on screen it's runes. A `len()` in rendering code is almost certainly a bug; a `RuneCountInString` used as a buffer offset is almost certainly a corruption.

### §1.6 Project Organization
- Name a package for its domain — never `types`/`utils`/`helpers`/`common`/`misc`.
- One primary type per file; decompose any file past 500 LoC.

### §1.7 One Value, One Meaning
Carry validity out of band — a `Valid` bool / option type in memory, `NULL` + `sql.NullXxx.Valid` in the database — never a `-1`/`""`/`0` sentinel that one missed check turns into real data:
```go
var clean sql.NullInt64 // ✓ .Valid answers "is there a clean point?"; .Int64 is the point
dirty := !clean.Valid || journalPos != clean.Int64
```

---

## §2 Component Architecture

### §2.1 State Residency
**The component that RENDERS a piece of state OWNS that state on its Model.** Litmus: if deleting the child's `View()` call would make a parent field dead code, the field belongs on the child.

### §2.2 Component Contract
Expose concrete methods — not Go interfaces:
```go
func New(keys keymap.Bindings, st styles.Styles) Model // constructor with deps
func (m Model) Init() tea.Cmd
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd)
func (m Model) View() string           // pure render
func (m Model) SetSize(w, h int) Model // accept allocated dimensions
func (m Model) Height() int            // parents query it, never hardcode it
// a focus-managing component also exposes: SetFocused(bool) Model
```

### §2.3 Pages vs Components
Pages hold children by value, route focus, translate cross-component messages, allocate layout, orchestrate I/O, and handle global keys. Components own their rendering state, accept `SetSize`, and handle their scoped keys when focused. When a page's `View()` composes several sub-views into a logical unit (breadcrumb + viewport = editor pane), extract that unit into a component.

### §2.4 Message Ownership
Define a message in the **producer's** package; a Cmd factory lives with its single-consumer message — the page calls the factory, it doesn't implement the I/O:
```go
case filetree.FileSelectedMsg:              // page translates and forwards
    return m, editor.LoadFileCmd(msg.Path)  // editor defines + handles its own load
```
Keep internal timer/expiry messages unexported in the owning component; export only the final result message (e.g. `ConfirmQuitMsg`).

### §2.5 Dependency Injection
Pass `keymap.Bindings` and `styles.Styles` by value via constructors, top-down. Pass `context.Context` / `*log.Logger` into the `tea.Cmd` factory that needs them (§6) — keep both off every Model. Components import neither pages nor domain packages.

---

## §3 Keybindings

### §3.1 One Key, One Binding
All bindings live in `keymap.Bindings` — the single source of truth. Before adding a key, scan every `key.WithKeys(...)` in `Default()`; one physical keypress resolves to exactly one logical action.

### §3.2 Chords
Own a chord/multi-press state machine in the component that **renders the confirmation feedback**; export only the completion message. The footer owns `^C^C`: first press arms the pending state and starts an expiry-timer Cmd, second press emits `ConfirmQuitMsg{}`; the page handles only `case footer.ConfirmQuitMsg: return m, tea.Quit`. A page never matches chord-sequence bindings itself — it forwards the `KeyPressMsg`.

### §3.3 Focus-Scoped Routing
Handle global keys FIRST in the page and stop them there (return early / consumed flag), then forward the message to children; each child gates on `m.focused`:
```go
case tea.KeyPressMsg:
    if !m.focused { return m, nil } // component ignores keys when unfocused
```

### §3.4 Matching Order
`key.Matches` runs sequentially: chord completions → global actions → contextual actions → fallthrough to focused children.

---

## §4 Layout & Dimensions

### §4.1 Query the Owner
Intrinsic sizes come from the child; extrinsic space is allocated by the parent via `SetSize`:
```go
contentH := totalH - m.footer.Height() // ✓ query the component
// const footerH = 1                    ✗ magic number for a child's dimension
```

### §4.2 recalcLayout
Do all dimension math in one `recalcLayout()` method, called from `Update` on `tea.WindowSizeMsg` OR any structural change (pane toggled, tab added/removed): query children's intrinsic sizes → compute allocations → `SetSize(w,h)` every child.

### §4.3 lipgloss v2 is Border-Box
`Width(n)`/`Height(n)` include borders and padding. Subtract the frame ONCE — in `recalcLayout()` when computing child (inner) dimensions, via `GetHorizontalFrameSize()`/`GetVerticalFrameSize()`; the border style in `View()` gets the OUTER dimension. Subtracting in both places double-subtracts the frame.

### §4.4 Single-Owner Clamping
`Height(n)` pads (a minimum), `Width(n)` wraps overflow; `MaxHeight(n)`/`MaxWidth(n)` truncate. The component that received `SetSize(w,h)` makes its own `View()` fit — `MaxWidth(m.width).MaxHeight(m.height)` as the outermost style. A page trusts a sized child's `View()` and never re-clamps it (re-clamping masks bugs); a page's own `Height`/`Width` on layout wrappers is its own box-sizing, not child-clamping.

### §4.5 Rendering Math Counts Runes
Every calculation that affects what the user sees uses `utf8.RuneCountInString` (§1.5):
```go
cellEnd := cellStart + utf8.RuneCountInString(cellText) // ✓ byte loops break on CJK/emoji
```

---

## §5 The Elm Cycle

### §5.1 Value Receivers — No Exceptions
`Init`, `Update`, `View` use value receivers. Where a library demands a pointer-based `tea.Model`, wrap with a thin adapter at the boundary rather than infecting the component tree.

### §5.2 Pure View
`View()` is a pure function of Model state: no assignment that outlives the call, no I/O, no mutating child method (`SetWidth`/`SetHeight`), no `tea.Cmd`.

### §5.3 Non-Blocking Update
Keep `Update()` and `Init()` non-blocking. File I/O, network, and timers each become a `tea.Cmd` returning a typed result `Msg` (a timer is a Cmd that sleeps and returns an expiry Msg).

### §5.4 Mutate Sync State Directly; Cmd Only for I/O
Use a `tea.Cmd` only for work that leaves the goroutine. When the page knows the intent and has the data, it acts directly through child methods:
```go
m.opentabs = m.opentabs.SelectIndex(idx)      // ✓ direct child mutation
cmds = append(cmds, editor.LoadFileCmd(path)) // ✓ only the I/O is a Cmd
// func() tea.Msg { return TabSwitchIndexMsg{idx} } ✗ self-message: +1 frame, leaks internals
```
Components expose query + mutation methods (`PathAt`, `SelectIndex`, `OpenFile`, …) for the page to orchestrate; they emit a message only for a user-initiated action the page can't anticipate (e.g. Enter on a focused item).

### §5.5 Cmd Closure Safety
Capture model-derived values into locals BEFORE the closure — it runs asynchronously, so the snapshot at creation is what matters (§1.2 example). Reading `m.field` inside the closure is a race.

### §5.6 Cmd Forwarding & Batching
After every `child.Update(msg)`, accumulate the returned Cmd and return `tea.Batch(cmds...)`. `tea.WindowSizeMsg` reaches every child that stores dimensions:
```go
m.editor, cmd = m.editor.Update(msg); cmds = append(cmds, cmd)
```

### §5.7 Viewport Scroll Preservation
`viewport.SetContent()` is scroll-destructive — capture `AtBottom()`/`YOffset` before, restore after.

---

## §6 Context & Async Operations

### §6.1 Off the Model
Keep `context.Context` and `*log.Logger` off every Model field (and off any shared/`Common` struct).

### §6.2 Into the Cmd Factory
Pass ctx into the `tea.Cmd` factory that does the I/O: check `ctx.Done()` inside, read through `vfs.FS` (§1.4.9), wrap errors with `%w`:
```go
func loadFileCmd(ctx context.Context, fsys vfs.FS, path string) tea.Cmd
```

### §6.3 Cancellation
Store a `context.CancelFunc` on the Model to cancel a superseded operation (watchers, long searches): cancel the previous → `context.WithCancel` → keep the new cancel → return the Cmd.

---

## §7 Page Transitions & Routing

### §7.1 The Router Holds No Rendering State
`app.go`'s Model is a router: it holds the pages by value plus an active-page discriminant, and delegates everything else.

### §7.2 Page Lifecycle
A page's `Init()` runs when it becomes active. Backgrounded pages retain state but receive no messages; only the active page's `View()` is called.

### §7.3 Inter-Page Communication
Pages communicate via messages routed through the top-level app (`case editor.OpenSettingsMsg: m.activePage = pageSettings; return m, m.settings.Init()`).

---

## §8 Testing

### §8.1 Mandatory Invariant Classes
Changing TUI behavior? Add tests for the affected class:

| Class | Assert |
|---|---|
| Render Purity | `View()` N times → identical output, no side effects |
| Layout | resize small/large → no overlapping bounds, correct child dimensions |
| Scroll Stability | set content while scrolled → offset preserved (or intentionally reset) |
| Async I/O | success AND failure msgs → correct transitions (missing file, permissions) |
| Key Routing | focused component gets keys; unfocused ignores; no binding collisions |

### §8.2 Test Patterns
Synchronize with a deterministic clock, never `time.Sleep`. Test `Update` → `Model` directly: feed messages, assert fields. For a Cmd: execute it, assert the resulting `Msg` type and contents. Tests exercise real input paths (`tea.KeyPressMsg` through the resolver), not just setter APIs.

---

## §9 (retired)
The former pitfalls tables; their examples are folded into the articles above.

---

## §10 Organization & Imports
- Components import neither pages nor sibling components — two components talk via messages routed through their parent page.
- Pages import components, never a child's implementation-detail packages (a page importing `viewport` signals a missing editor component).
- `styles` and `keymap` are leaf packages — they import nothing from `pkg/ui/`.

(Directory map: `CLAUDE.md`.)

---

## §11 Pre-Merge Checklist

Verify mechanically before completing a change:

- [ ] Data-integrity failures halt with a surfaced error; async/external edits are clamped, an empty reset never applies as a destructive replace, and a rejected clamp is surfaced. (§1.3)
- [ ] Every error is handled or annotated `// fire-and-forget: …`; errors stay `error`. (§1.3)
- [ ] User-content writes go through `atomicfile.Write`; unsaved work goes to the recovery store, never a debounced write to the destination. (§1.4.1, §1.4.2)
- [ ] Crash-recovery state is durable; recovery identity is keyed to inode+device. (§1.4.3, §1.4.6)
- [ ] Every destructive transition on a dirty buffer prompts or preserves; dirty is recomputed from the store on each transition. (§1.4.4, §1.4.8)
- [ ] Bytes stay verbatim — no normalized line endings, trailing newline, BOM, or encoding. (§1.4.5)
- [ ] All runtime FS access goes through the injected `vfs.FS`. (§1.4.9)
- [ ] Displaced bytes are captured as a durable blob before removal on every overwrite path. (§1.4.10)
- [ ] Edit/cursor offsets in **bytes** (`len`); display widths in **runes** (`utf8.RuneCountInString`). (§1.5)
- [ ] No magic sentinel for "invalid/absent"; DB absence is `NULL` via `sql.NullXxx.Valid`. (§1.7)
- [ ] Reusable slices reset with `[:0]`; no file exceeds 500 LoC. (§1.1, §1.6)
- [ ] `Init`/`Update`/`View` use value receivers; `View()` is pure. (§5.1, §5.2)
- [ ] Every child Cmd is accumulated via `tea.Batch`; `tea.WindowSizeMsg` reaches every sized child. (§5.6)
- [ ] Every `tea.Cmd` closure captures locals, not model fields. (§5.5)
- [ ] No Model holds `context.Context` or `*log.Logger`. (§6.1)
- [ ] Rendered state lives on the component that renders it; dimensions come from child methods, not consts. (§2.1, §4.1)
- [ ] Each key string maps to exactly one binding; chord state lives in the component rendering the feedback. (§3.1, §3.2)

---

## §12 Standing Decisions

The normative core of retired design records — each anchored to a grep-resolvable symbol:

- **SyncFunc is the single seam for display-content sync** — `buf`/cursors/focus/width into a `SyntaxMap`/`SyntaxSnapshot` (`WithSyncFunc`) — a concrete function value, never a Go interface or multiple hooks. It is not the ONLY extension seam `textedit` exposes: `CellBuilderFunc`/`ImageRowFunc` (next bullet) are the separate, render-time hooks that style and image-row-render SyncFunc's OUTPUT — each still a single concrete function value per concern, never a Go interface or per-token dispatch, just not the SAME seam as SyncFunc.
- **Styling is derived at render time, not pre-baked**: `CellBuilderFunc` (`textedit.Model.RenderView`/`renderCells`) is the seam between one `display.DisplaySpan`'s Kind/Marks/`LinkRole()` and the `lipgloss.Style` it renders with — a concrete function value, never a Go interface or per-token dispatch inside `textedit` itself. `textedit`'s own default (`defaultCellBuilder`) renders every span with a zero-value base style and zero TokenKind dispatch; `markdownedit.spanToCellsStyled` is the ONE builder that maps Kind/Marks/LinkRole to style, and is what markdown syntax highlighting actually is. `ImageRowFunc` is the parallel per-line seam for image-embed rows (Kitty/iTerm2 placeholder cells), closing over `markdownedit`'s own image/terminal-capability state — `textedit` itself has no images and never sets it.
- **The workspace owns all file I/O and docstate persistence**; `textedit` keeps no undo stack — the journal drives undo/redo through textedit's apply/set primitives.
- **Two durable roles, one store**: the per-document journal (`events`) and content-addressed snapshots, both in the on-disk SQLite DB.
- **Undo is two-tier**: comfortable ⌘Z plus a time-travel scrubber (`MoveUndoPos`/`UndoPeek`/`RedoPeek`), reconstructing from journal + snapshots.
- **One document, one event stream**: the journal is keyed by `doc_id`; ⌘Z routes by focus (`undoTarget()`); the title field is unjournaled — a rename is one atomic bind.
- **Autosave targets the recovery store**, never the destination file (§1.4.2); the dirty indicator and `^C^C` quit-guard are current, intentional behavior.
- **Disk is an observed participant**: every look at disk (`Load`, `Probe`, each `Materialize` step) records an `observations` row; sync state and the merge ancestor are derived fresh from observations by journal position, never cached.
- **Per-workspace store, SQLite-native concurrency, session-scoped journal**: `docstate.Open(workDir)` opens `workDir/.rune/rune.db` — different workspaces never share a database, so unrelated vaults never contend. Concurrent opens of the *same* workDir (two rune windows on the same file) are arbitrated by SQLite's own locking (`_txlock=immediate` + `_busy_timeout`), not a custom flock, AND by giving every journaled edit a process identity: each `Store` construction gets its own `sessions` row (`Store.sessionID`), and `AppendEdit`/undo-redo/coalescing/`RecoverDocument` all scope to `(doc_id, session_id)`, so a session's own journal position can never be corrupted by a *different* session's concurrent keystrokes to the same doc — the journal race a machine-global flock used to prevent by refusing outright is now prevented by construction instead. Ancestor ELIGIBILITY (`ancestorAt`, the 3-way-merge baseline) is scoped the same way — a *different* session's save/load/resolve observation must never silently become this session's ancestor, or a later `[M]erge` would discard their change with no conflict markers shown. Reading the freshest disk fact (`newestObservation`, "theirs") stays deliberately UNSCOPED by session: any session's disk fact is everyone's disk fact, and that is what lets two sessions on the same file reconcile through the ordinary conflict-guard/3-way-merge machinery already built for "rune vs. an external tool" — no refusal, no locking, two windows on the same file just work. This all depends on an invariant every mutating `Store` method must uphold: read whatever state you use to decide what to write *inside* the same transaction you write in — a read-then-later-write split reopens exactly the race the flock used to paper over (found and fixed once, in `openPathByName`/`openPathByInode`, before this was safe to rely on; re-applied when undo position/CAS baseline moved from `documents` to the per-session `session_documents` table).
- **SQLite is a disposable shadow of `.md` truth**: schema migration is drop-on-version-mismatch — a permanent policy, not a stopgap.
- **Help is a virtual read-only document** tab whose keybinding reference is generated by reflection over `keymap.Bindings` — a hand-maintained key list may not exist.
- **In-file search is a textedit search bar** with durable fuzzy history (`AppendSearchQuery`/`SearchHistory`).
