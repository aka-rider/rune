# WP0 — Test Infrastructure & Shared Helpers

## Scope

Audit and harden the existing `internal/editortest/` package. If files already exist, preserve their public API unless this workpackage explicitly changes it; do not rewrite working helpers just to match old prose.

## Dependencies

None (can run in parallel with WP1, WP5, WP6)

## Deliverables

### `internal/editortest/notation.go`

- `ParseState(notation string) (TestState, error)` and `FormatState(s TestState) string`
- Supports `|` (cursor), `[text]` (forward selection), `]text[` (backward selection), multi-cursor, escape sequences (`\|`, `\[`, `\]`)
- All positions are UTF-8 byte offsets into `Content`, not rune indexes. Add Unicode tests for `é`, `中`, mixed ASCII/Unicode, and escaped markers.
- Empty notation and notation with no cursor marker are invalid inputs and must return clear errors. Empty buffers are represented as `"|"`.

```go
package editortest

type TestState struct {
    Content  string
    Cursors  []CursorState  // sorted by offset
}

type CursorState struct {
    Position int
    Anchor   int  // == Position if no selection
}

func ParseState(notation string) (TestState, error)
func FormatState(s TestState) string
```

### `internal/editortest/clock.go`

- Deterministic `Clock` value type

```go
type Clock struct {
    now time.Time
}

func NewClock() Clock { return Clock{now: time.Date(2024, 1, 1, 0, 0, 0, 0, time.UTC)} }
func (c Clock) Now() time.Time { return c.now }
func (c Clock) Advance(d time.Duration) Clock { return Clock{now: c.now.Add(d)} }
```

### `internal/editortest/golden.go`

- Golden file comparison helper
- Read/write/update mode via `-update` flag
- Uses `t.Helper()` in assertions

### `internal/editortest/diff.go`

- Unified diff output for test failure messages
- Line-by-line comparison with context

### `internal/editortest/notation_test.go`

- Round-trip: `ts, err := ParseState(s); err == nil; FormatState(ts) == s` for all valid notations
- Multi-cursor parsing with mixed selections
- Escape character handling
- Edge cases: empty buffer (`"|"`), cursor at start/end, adjacent cursors, Unicode byte offsets

## Constraints

- No external test dependencies (no testify)
- No imports from `pkg/editor/` packages (prevents circular imports)
- Works only with plain data types (string content + int offsets)
- All files under 500 LoC

## QA Gates

These gates protect all downstream workpackages. Notation errors corrupt every test that uses them.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | `ts, err := ParseState(s); err == nil; FormatState(ts) == s` for all valid notations (round-trip identity) | Broken notation = every downstream spec test asserts wrong state |
| 2 | `ParseState(FormatState(ts)) == ts` for all valid TestState values (inverse identity) | Test harness produces wrong expected values |
| 3 | Golden helper detects single-byte difference between actual and expected | Regressions slip through undetected |
| 4 | ParseState rejects malformed notation with clear error (unclosed `[`, orphan `]`, empty notation, no cursor marker) | Silent mis-parse produces garbage test states that pass incorrectly |
| 5 | Multi-cursor notation preserves offset ordering after parse (cursors[i].Position ≤ cursors[i+1].Position) | Downstream merge/adjust tests get wrong input ordering |
| 6 | Unicode notation returns byte offsets: `aé|中` reports position 3, not rune index 2 | Editor commands operate in byte offsets; rune offsets corrupt UTF-8 positions |

**Testing approach:** Table-driven with explicit edge cases + property test (random valid TestState → format → parse → compare).

## Verification

```bash
go test ./internal/editortest/... -v
```

- All notation round-trips pass
- Clock advances deterministically
- Golden helper creates/updates/compares files correctly
