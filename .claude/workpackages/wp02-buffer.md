# WP2 — Buffer (Immutable String Implementation)

## Scope

`pkg/editor/buffer/`

## Dependencies

- WP1 (coords types for `BufferPoint`)

## Deliverables

### `pkg/editor/buffer/buffer.go`

Full public API from spec §A:

```go
package buffer

type Edit struct {
    Start  int
    End    int
    Insert string
}

type AppliedEdit struct {
    Start   int
    End     int
    Deleted string
    Insert  string
}

type Buffer struct { /* internal fields */ }

// Construction
func New(content string) Buffer
func FromBytes(content []byte) (Buffer, error)  // validates UTF-8; error on invalid
func (b Buffer) Empty() bool

// Mutation — all return new Buffer (value semantics, snapshot-safe)
func (b Buffer) Insert(offset int, text string) Buffer
func (b Buffer) Delete(start, end int) Buffer
func (b Buffer) Replace(start, end int, text string) Buffer
func (b Buffer) ApplyEdits(edits []Edit) (Buffer, []AppliedEdit, error)

// Access
func (b Buffer) Len() int
func (b Buffer) Slice(start, end int) string
func (b Buffer) Byte(offset int) byte
func (b Buffer) RuneAt(offset int) (rune, int)
func (b Buffer) Line(n int) string
func (b Buffer) LineCount() int
func (b Buffer) LineStart(n int) int
func (b Buffer) LineEnd(n int) int
func (b Buffer) OffsetToLineCol(offset int) coords.BufferPoint
func (b Buffer) LineColToOffset(bp coords.BufferPoint) int
func (b Buffer) Version() uint64
func (b Buffer) Content() string
```

### `pkg/editor/buffer/lineindex.go`

- Line-start offset cache (`lineStarts []int`)
- Incremental maintenance on edit (spec algorithm: find affected range, count newlines removed/added, shift subsequent entries)
- O(1) `LineStart(n)`, O(log N) `OffsetToLineCol()` via binary search

### `pkg/editor/buffer/buffer_test.go`

**Layer 1 — Invariant/Fuzz Tests:**
- `FuzzBufferSnapshotImmutability`: After edit, original Buffer value unchanged
- `FuzzBufferBatchEquivalence`: ApplyEdits in one call == applying same edits individually

**Layer 2 — Spec Scenarios:**
- Insert at beginning, middle, end
- Delete range, empty delete
- Replace range
- Multi-edit batch (descending order)
- UTF-8 validation (reject invalid bytes)
- Descending-order enforcement (error on ascending edits)
- No-overlap enforcement (error on overlapping edits)
- Line index: LineCount, Line, LineStart, LineEnd after edits
- OffsetToLineCol / LineColToOffset round-trip
- Version increments on each edit

## Constraints

- First implementation uses immutable string internally (not piece table)
- `ApplyEdits` MUST enforce: edits sorted descending by Start, no overlaps
- `FromBytes` rejects invalid UTF-8 with wrapped error
- Zero value is meaningful (empty buffer)
- No external dependencies beyond stdlib
- All files under 500 LoC

## QA Gates

These gates protect WP3 (cursor bounds depend on Len), WP4 (history inverse depends on ApplyEdits), WP7 (display depends on line index), and all downstream editing.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | **P7:** `buffer.Len() == len(buffer.String())` after ANY sequence of Insert/Delete/Replace/ApplyEdits | Every cursor bounds check in WP3+ uses Len(). Wrong Len = out-of-bounds cursors = panic or corruption |
| 2 | **P2:** No operation produces invalid UTF-8 from valid UTF-8 input (fuzz: random valid UTF-8 content + random valid UTF-8 insert → `utf8.ValidString(result)`) | Invalid UTF-8 corrupts file on save, crashes display pipeline, produces mojibake |
| 3 | Line index consistency: `OffsetToLineCol(LineColToOffset(bp)) == bp` for all valid BufferPoints | Wrong line index breaks cursor positioning in WP9, display row mapping in WP7 |
| 4 | `ApplyEdits` with N non-overlapping descending edits produces correct content (each edit's text appears at the right position in output) | Multi-cursor editing (WP11) depends entirely on batch-edit correctness |
| 5 | `ApplyEdits` rejects overlapping edits and ascending-order edits with error | Silent acceptance of invalid input causes data corruption that surfaces much later |
| 6 | `FromBytes` rejects `[]byte{0xff, 0xfe}` (invalid UTF-8) with non-nil error | Loading corrupt files without error = user edits garbage thinking it's valid |

**Testing approach:** P2 and P7 via fuzz (60s in CI). Gates 3-6 via table-driven scenarios.

## Verification

```bash
go test ./pkg/editor/buffer/ -v
go test ./pkg/editor/buffer/ -fuzz FuzzBufferSnapshotImmutability -fuzztime 30s
go test ./pkg/editor/buffer/ -fuzz FuzzBufferBatchEquivalence -fuzztime 30s
```
