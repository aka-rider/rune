# WP8 — Editor Component (Core Editable Model)

## Scope

`pkg/ui/components/editor/` — migrate the current read-only viewport wrapper to a full editable editor in two phases:

- **WP08A — state machine:** model fields, file messages, command dispatch, `applyOperation`, undo/redo stubs, display sync, dirty tracking.
- **WP08B — view and wiring:** rendering, cursor/selection overlay, sizing, breadcrumb integration, app/workspace constructor migration.

Do not attempt all UI polish and command logic in this workpackage. WP09-WP18 fill command behavior by category.

## Dependencies

- WP2 (buffer), WP3 (cursor), WP4 (history), WP5 (command registry), WP6 (keybind resolver), WP7 (display pipeline)

## Deliverables

### `pkg/ui/components/editor/editor.go` (rewrite)

Replace current viewport wrapper with full editor model:

```go
type Model struct {
    // Document state
    buf          buffer.Buffer
    cursors      cursor.CursorSet
    history      history.UndoStack
    dirty        bool
    savedContentHash string
    activeSave   SaveIdentity
    filePath     string
    softWrap     bool
    indent       IndentConfig

    // Display pipeline
    syntaxMap    display.SyntaxMap
    wrapMap      display.WrapMap
    snapshot     display.DisplaySnapshot
    syntaxSnap   display.SyntaxSnapshot
    wrapSnap     display.WrapSnapshot

    // Input handling
    resolver     keybind.Resolver
    registry     command.Registry

    // Layout
    viewport     ViewportState
    breadcrumb   breadcrumb.Model
    keys         keymap.Bindings
    styles       styles.Styles
    width        int
    height       int
    focused      bool
}

type ViewportState struct {
    TopRow    int
    ScrollCol int
}

type IndentConfig struct {
    UseTabs bool
    TabSize int
}

type SaveIdentity struct {
    Path        string
    RequestID   string
    ContentHash string
    InFlight    bool
}
```

Dirty state must compare current content identity to `savedContentHash` (or an equivalent saved-content snapshot). Do not use only a buffer-version counter; undoing back to saved bytes must make `IsDirty()` false.

Component contract (value receivers):
```go
func New(keys keymap.Bindings, st styles.Styles, reg command.Registry, resolver keybind.Resolver) Model
func (m Model) Init() tea.Cmd
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd)
func (m Model) View() string
func (m Model) SetSize(w, h int) Model
func (m Model) Height() int
func (m Model) SetFocused(focused bool) Model
```

Content management:
```go
func (m Model) SetContent(path string, content []byte) Model
func (m Model) Content() string
func (m Model) IsDirty() bool
func (m Model) FilePath() string
func (m Model) WantsModalInput() bool  // true when an internal overlay must receive keys before workspace globals
func (m Model) StartSave() (Model, SaveIdentity, tea.Cmd)
```

Query methods:
```go
type CursorInfo struct {
    Line         int
    Col          int
    WordCount    int
    Dirty        bool
    ChordPending string
}
func (m Model) CursorInfo() CursorInfo
```

### `pkg/ui/components/editor/apply.go`

Core orchestration methods from spec §E:
- `applyOperation(op command.Operation, kind history.EditKind, now time.Time) Model`
- `applyUndo() (Model, tea.Cmd)`
- `applyRedo() (Model, tea.Cmd)`
- `syncDisplay() Model`
- `scrollToCursor() Model`
- `scrollPreservingAnchor(oldSnapshot, newSnapshot) Model`
- `dispatchOperation(result command.Result, cmdName string, now time.Time) (Model, tea.Cmd)`
- `clampCursorsToViewport() cursor.CursorSet` — clamp cursor positions to visible range after scroll
- `editKindFromCommand(cmdName string) history.EditKind` — maps command names to EditKind
- `isPrintable(msg tea.KeyPressMsg) bool` — checks if key is a printable character
- `chordTimeoutCmd() tea.Cmd` — returns a Cmd that fires `chordTimeoutMsg` after 1500ms

### Command files

**Architecture note:** The `command.Registry` is immutable after `Build()`. Pre-register all command names in category files so later workpackages edit small, owned files instead of one shared hot spot. The registry is built once at app init.

- `commands_registry.go` — `RegisterCommands(builder command.Builder) (command.Builder, error)` and shared command helpers
- `commands_nav.go` — WP09 navigation/scroll/select command implementations
- `commands_edit.go` — WP10 basic editing command implementations
- `commands_multi.go` — WP11 multi-cursor command implementations
- `commands_history.go` — WP12 undo/redo command implementations
- `commands_clipboard.go` — WP15 clipboard command implementations
- `commands_mouse.go` — WP16 mouse helper entry points if command-dispatched
- `commands_find.go` — WP18 find/replace stubs
- Define all command names with stub `Execute` functions (return `OperationNone`) in their category file until the owning WP replaces them.

### Messages (defined in this package per §2.4):

```go
type FileLoadedMsg struct { Path string; Content []byte }
type FileLoadErrorMsg struct { Path string; Err error }
type FileSavedMsg struct { Path string; RequestID string; SavedContentHash string }
type FileSaveErrorMsg struct { Path string; RequestID string; Err error }
type ContentChangedMsg struct { Path string; Dirty bool }
```

Cmd factories:
```go
type SaveRequest struct {
    Path      string
    Content   string
    RequestID string
    ContentHash string
}

func LoadFileCmd(path string) tea.Cmd
func SaveFileCmd(req SaveRequest) tea.Cmd
```

Also register an explicit `file.save` command bound to `Cmd+S`; dirty guard save and user-triggered save must use the same save path.

`SaveFileCmd` must capture `SaveRequest` fields into locals before creating the `tea.Cmd` closure. Every save uses a non-empty `RequestID`. Dirty-guard saves use that ID so workspace can correlate completion with the pending guard action.

The editor tracks the active/latest save identity. A `FileSavedMsg` or `FileSaveErrorMsg` mutates editor save state only when `msg.Path == m.filePath`, `m.activeSave.InFlight == true`, and `msg.RequestID == m.activeSave.RequestID`. Stale completions for previous files or superseded requests are ignored except for surfacing an error if appropriate.

On matching `FileSavedMsg`, the editor records `savedContentHash = msg.SavedContentHash` and marks clean only when `hash(m.buf.Content()) == msg.SavedContentHash`. If the user edited after the save request was created, the save completion updates saved identity but must not mark the current buffer clean.

`file.save` is registered in the command registry, but its `Execute` function lives in the editor package. The editor's `CommandContext` must provide `FilePath`, `Buffer`, `NewRequestID`, and `HashContent` so `file.save` can return a structured `OperationSaveFile` with path/content/request ID/content hash. It must not hide save identity inside an opaque `tea.Cmd`.

`dispatchOperation` handles `OperationSaveFile` by consuming the exact save fields carried on the operation:

```go
func (m Model) startSaveRequest(req SaveRequest) (Model, SaveIdentity, tea.Cmd) {
    // set m.activeSave from req, return SaveFileCmd(req)
}

func (m Model) StartSave() (Model, SaveIdentity, tea.Cmd) {
    // snapshot path/content/hash/requestID into req, then call startSaveRequest(req)
}
```

`file.save` returns `OperationSaveFile` with the already-snapshotted path/content/request ID/content hash. `dispatchOperation` must call `startSaveRequest(reqFromOperation)` and must not generate a second request ID. Dirty guards call `StartSave()`, which snapshots current editor state and then uses the same `startSaveRequest` helper. Workspace code must not call `SaveFileCmd` directly.

`FileLoadedMsg` handling must call `buffer.FromBytes(msg.Content)`. If bytes are invalid UTF-8, emit `FileLoadErrorMsg` and leave the existing editor state intact.

### `View()` implementation:

- Renders plain text with cursor position indicator
- Selection highlighting (reverse video or configured style)
- Line numbers (optional)
- Hard `MaxWidth(m.width).MaxHeight(m.height)` clamping
- Focus gating in Update: ignore KeyPressMsg when `!m.focused`

### Keymap migration before WP10

Before editing commands are enabled, update `pkg/ui/keymap/keymap.go` so every physical key string has exactly one binding. Do not rely on context predicates to allow duplicate physical keys.

`pkg/ui/keymap` owns both UI component physical bindings and editor command bindings. Add a conversion API used during app startup:

```go
func (b Bindings) CommandBindings() ([]keybind.Binding, error)
func (b Bindings) AllPhysicalKeys() []string
func (b Bindings) ValidateNoPhysicalKeyCollisions() error
```

`CommandBindings` converts the central physical key declarations into resolver bindings with command names and `When` predicates. Editor packages must not define separate physical key lists that bypass keymap collision tests.

Command registry metadata is the source of truth for command availability. During `ui.NewApp()` startup validation:

1. Build the immutable command registry.
2. Call `keys.ValidateNoPhysicalKeyCollisions()` over all UI and command physical keys.
3. Build keymap command bindings.
4. For every binding, verify `registry.Get(binding.Command)` succeeds.
5. Verify any binding `When` predicate matches the registered command's `When` predicate, or require binding `When` to be empty and copy the command predicate into the resolver binding.
6. Fail startup with wrapped context on duplicate physical keys, missing commands, divergent predicates, malformed predicates, or duplicate chord sequences.

Add startup tests for missing command names and mismatched predicates.

| Physical key | Single owner after migration | Notes |
|---|---|---|
| `backspace` | `edit.delete-left` | Move footer help expansion to `?`. |
| `tab` | `edit.indent` / editor-local overlay tab handling | Remove workspace focus cycling from `tab`; use explicit focus keys such as `ctrl+x` and `ctrl+e`. |
| `shift+tab` | `edit.outdent` | Do not reuse for focus cycling. |
| `enter` | one routed primary-action binding | File tree/open tabs may interpret focused primary action as open/select; editor interprets it as newline; find overlay invokes its stub next action. The physical key appears once in keymap. |
| `esc` | one routed cancel binding | Editor priority: find overlay close, multi-cursor/selection collapse, then parent fallback. Move zen mode to `ctrl+o`. |
| printable letters (`j`, `k`, `g`, `G`, `b`, `f`, `u`, `d`, etc.) | text insertion when editor focused | Remove legacy printable navigation aliases from editor-facing keymap bindings. Components may keep non-editor local behavior only if it does not create duplicate central key strings. |

Concrete routed-key API:

- Rename/replace the existing `Select` binding with `PrimaryAction` (`enter`) and the existing `ZenMode`/escape binding with `Cancel` (`esc`).
- `CommandBindings()` must not emit separate physical resolver bindings for `enter` or `esc`.
- Workspace and focused components route `PrimaryAction`/`Cancel` by state and focus. When editor owns the routed action, it still executes the corresponding command through the registry (`edit.newline`, `multicursor.escape`, or find-overlay stub action); it simply bypasses physical-key resolver lookup to avoid duplicate physical bindings.
- Filetree/opentabs use `PrimaryAction` for open/select. Editor uses `PrimaryAction` for newline only when no modal overlay owns it. Find overlay handles overlay-local Enter first.
- Editor uses `Cancel` for find overlay close, then multi-cursor/selection collapse. Workspace fallback behavior must not use `esc` for zen mode.

Add an editor-focus test that ordinary printable letters insert text and do not trigger legacy navigation/page commands.

Add `pkg/ui/keymap` collision tests in this workpackage, before WP10 editing keys land:

```go
func TestNoKeybindingCollisions(t *testing.T) {
    // Extract all physical key strings from keymap.Default().AllPhysicalKeys()
    // Fail if any key string appears in more than one binding
}
```

Also add focused-editor ownership tests for `Tab`, `Esc`, and legacy printable aliases. WP19 re-runs these as final gates.

### Wiring changes:

- `pkg/ui/app.go`: Add `NewApp() (Model, error)`. It builds immutable registry, calls `keys.ValidateNoPhysicalKeyCollisions()`, calls `keys.CommandBindings()`, validates every binding against the registry, calls `keybind.NewResolver(...)`, wraps any errors with context, and passes the validated resolver to workspace/editor.
- `cmd/rune/main.go`: call `ui.NewApp()`, print startup errors to stderr, and exit non-zero before starting Bubble Tea.
- `pkg/ui/pages/workspace/workspace.go`: Pass registry and validated resolver to editor constructor.
- `pkg/ui/keymap/keymap.go`: Resolve backspace conflict before WP10 by moving footer help expansion to a different physical key. Do not rely on context-gating duplicate key strings.

### `pkg/ui/components/editor/editor_test.go`

- Integration: create editor, set content, verify View() output
- Focus gating: unfocused editor ignores key presses
- SetSize: output respects dimensions
- File load: SetContent updates buffer and resets state

## Constraints

- CLAUDE.md §2.1: editor owns all rendering state
- CLAUDE.md §4.4: View() uses MaxWidth/MaxHeight for clamping
- CLAUDE.md §5.1: value receivers only
- CLAUDE.md §5.5: Cmd closures capture locals, not model fields
- All files under 500 LoC

## QA Gates

This is the convergence point. These gates validate that the wiring of WP2-WP7 into a functioning editor doesn't violate any of their individual guarantees.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | Properties P1-P8 hold after operations through editor.Model (SetContent + simulated edits + simulated cursor moves) | Wiring bug in applyOperation breaks invariants that individual packages maintain |
| 2 | `View()` output width ≤ allocated SetSize width; output height ≤ allocated SetSize height | Editor overflows into adjacent TUI components → garbled display |
| 3 | `FileLoadedMsg` → `buffer.String() == file content`, cursors at offset 0, `IsDirty() == false` | File loads but editor shows stale content or dirty flag is wrong |
| 4 | Focus gating: `SetFocused(false)` → KeyPressMsg ignored (content unchanged after any key sequence) | Unfocused editor silently modifies buffer → invisible data corruption |
| 5 | Cmd closures returned from Update do NOT capture model fields (static analysis or test: returned Cmd executes correctly after model changes) | Race condition between async Cmd execution and model mutation |
| 6 | Edit after load → dirty, save → clean, edit then undo back to saved bytes → clean | Version-only dirty tracking leaves editor dirty even when content matches saved file |
| 7 | Stale save completions are ignored: save A, discard/open B, then A completion arrives | Old async save mutates dirty state for the wrong open file |
| 8 | Duplicate/out-of-order saves only let the active/latest matching request update saved identity | Earlier save completion marks newer dirty content clean |

**Testing approach:** Gates 1-4 via integration tests (create editor, feed messages, assert). Gate 5 via code review discipline (enforced by CLAUDE.md §5.5).

## Verification

```bash
go build ./pkg/ui/components/editor/...
go build ./...  # full project compiles with wiring changes
go test ./pkg/ui/components/editor/ -v
go test ./pkg/ui/keymap/ -run TestNoKeybindingCollisions -v
```
