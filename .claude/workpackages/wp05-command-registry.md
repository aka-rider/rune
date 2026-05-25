# WP5 — Command Registry

## Scope

`pkg/command/`

## Dependencies

- WP2 (buffer — `Buffer`, `Edit`, `AppliedEdit` types used in `CommandContext` and `Operation`)
- WP3 (cursor — `CursorSet` type used in `CommandContext` and `Operation`)

## Deliverables

### `pkg/command/command.go`

Types from spec §K:

```go
package command

type ArgSpec struct {
    Name     string
    Type     string  // "string", "int", "bool"
    Required bool
}

type CommandContext struct {
    Buffer    buffer.Buffer
    Cursors   cursor.CursorSet
    FilePath  string
    Args      map[string]any
    Now       time.Time
    NewRequestID func() string
    HashContent  func(string) string
    Selection func() string
    LineCount func() int
}

type OperationKind int
const (
    OperationNone OperationKind = iota
    OperationMoveCursors
    OperationEditBuffer
    OperationScroll
    OperationClipboard
    OperationHistory
    OperationSaveFile
)

type Operation struct {
    Kind      OperationKind
    Edits     []buffer.Edit
    Cursors   cursor.CursorSet
    ScrollDY  int
    ScrollDX  int
    SavePath        string
    SaveContent     string
    SaveRequestID   string
    SaveContentHash string
}

type Result struct {
    Operation Operation
    Cmd       tea.Cmd
    Err       error
}

type CommandFn func(ctx CommandContext) Result

type Command struct {
    Name     string
    Category string
    Title    string
    Execute  CommandFn
    Args     []ArgSpec
    When     string
}
```

### `pkg/command/registry.go`

```go
type Builder struct { /* commands map[string]Command */ }
type Registry struct { /* commands map[string]Command — immutable after Build */ }

func NewBuilder() Builder
func (b Builder) Register(cmd Command) (Builder, error)  // copies map (aliasing safety)
func (b Builder) Build() Registry
func (r Registry) Get(name string) (Command, bool)
func (r Registry) Execute(name string, ctx CommandContext) Result
func (r Registry) Search(query string) []Command  // fuzzy match on Name/Title
func (r Registry) All() []Command
```

`pkg/command` may import Bubble Tea only for `tea.Cmd`. It must not import UI components, pages, or styles.

`Search` must be deterministic and dependency-free for this workpackage: case-insensitive substring matches on `Name` and `Title` first, then stable alphabetical order by `Name` for ties. Do not introduce a fuzzy-search dependency unless a later package explicitly chooses one.

### `pkg/command/registry_test.go`

- Register and Get by name
- Duplicate name returns error
- Execute dispatches to CommandFn and returns Result
- Search fuzzy matches (partial name, partial title)
- Builder aliasing safety: Register on copy doesn't affect original
- Build produces immutable Registry (subsequent Register on builder doesn't affect registry)

## Constraints

- `Register()` copies internal map before inserting (aliasing safety — Go maps are reference types)
- Registry is immutable after `Build()` — no registration after initialization
- Search order is stable for identical inputs
- No interfaces for single implementations
- Under 500 LoC per file

## QA Gates

These gates protect WP8+ (all command dispatch goes through Registry) and WP9-WP18 (every command is registered here).

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | After Build(), concurrent Get() calls never panic and always return consistent results | Editor crashes under normal multi-key operation (race in command lookup) |
| 2 | Register with duplicate Name returns error | Silent overwrite of existing command → user loses access to overwritten functionality |
| 3 | Builder aliasing safety: Register on a copied Builder does not affect the original | Shared builder state causes non-deterministic command availability |
| 4 | Execute dispatches to correct CommandFn and returns its Result | Wrong command fires for a keystroke → user presses delete but gets insert |

**Testing approach:** Table-driven for gates 2-4. Gate 1 via concurrent goroutine test (100 goroutines calling Get simultaneously).

## Verification

```bash
go test ./pkg/command/ -v
```
