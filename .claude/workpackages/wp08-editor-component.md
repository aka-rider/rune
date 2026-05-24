# WP8 — Editor Component (Core Editable Model)

## Scope

`pkg/ui/components/editor/` — rewrite from read-only viewport to full editable editor

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
    savedVersion uint64
    filePath     string

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
```

Component contract (value receivers):
```go
func New(keys keymap.Bindings, st styles.Styles, reg command.Registry) Model
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

### `pkg/ui/components/editor/commands.go`

**Architecture note:** The `command.Registry` is immutable after `Build()`. ALL command `Execute` functions are defined in `commands.go` within the editor package. WP9, WP10, WP11, WP12, WP15, WP16, WP18 implement the **logic** of these functions — they modify `commands.go` to replace stub implementations with real ones. The registry is built once at app init with all command names pre-registered. Workers for WP9-18 edit the same file to fill in implementations.

- Define all ~60 command names with stub `Execute` functions (return `OperationNone`)
- Command registration during initialization via `Builder`

### Messages (defined in this package per §2.4):

```go
type FileLoadedMsg struct { Path string; Content []byte }
type FileLoadErrorMsg struct { Path string; Err error }
type FileSavedMsg struct { Path string }
type FileSaveErrorMsg struct { Path string; Err error }
type ContentChangedMsg struct { Path string; Dirty bool }
```

Cmd factories:
```go
func LoadFileCmd(path string) tea.Cmd
func SaveFileCmd(path, content string) tea.Cmd
```

### `View()` implementation:

- Renders plain text with cursor position indicator
- Selection highlighting (reverse video or configured style)
- Line numbers (optional)
- Hard `MaxWidth(m.width).MaxHeight(m.height)` clamping
- Focus gating in Update: ignore KeyPressMsg when `!m.focused`

### Wiring changes:

- `pkg/ui/app.go`: Build immutable registry, pass to workspace
- `pkg/ui/pages/workspace/workspace.go`: Pass registry to editor constructor (3-arg migration)
- `pkg/ui/keymap/keymap.go`: Resolve backspace conflict (move footer help expansion to different key or context-gate)

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

**Testing approach:** Gates 1-4 via integration tests (create editor, feed messages, assert). Gate 5 via code review discipline (enforced by CLAUDE.md §5.5).

## Verification

```bash
go build ./pkg/ui/components/editor/...
go build ./...  # full project compiles with wiring changes
go test ./pkg/ui/components/editor/ -v
```
