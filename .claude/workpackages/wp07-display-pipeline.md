# WP7 — Display Pipeline (SyntaxMap + WrapMap + Snapshot)

## Scope

`pkg/editor/display/`

## Dependencies

- WP1 (coords types)
- WP2 (buffer — for Buffer access in SyntaxMap.Sync)
- WP3 (cursor — for CursorSet in SyntaxMap.Sync)

## Deliverables

### `pkg/editor/display/syntax_map.go`

```go
package display

type TokenKind int
const (
    TokenText TokenKind = iota
    TokenHeading
    TokenBold
    TokenItalic
    TokenStrikethrough
    TokenInlineCode
    TokenLink
    TokenImage
    TokenBlockquote
    TokenTaskList
    TokenHorizontalRule
    TokenCodeFence
    TokenTable
    TokenMathBlock
    TokenInlineMath
    TokenFrontmatter
    TokenCallout
    TokenListMarker
    TokenTag          // #tag syntax — always visible
    TokenRawURL       // bare URLs — always visible
    TokenHighlight    // ==text== — per-token reveal
)

type RevealState int
const (
    Rendered RevealState = iota
    Revealed RevealState = iota
)

type SyntaxSpan struct {
    Text        string
    Kind        TokenKind
    State       RevealState
    BufferStart int
    BufferEnd   int
}

type SyntaxLine struct {
    Spans []SyntaxSpan
}

type OffsetDelta struct {
    BufferOffset int
    Delta        int
}

type SyntaxSnapshot struct {
    Lines  []SyntaxLine
    Deltas []OffsetDelta
}

func (s SyntaxSnapshot) BufferToSyntax(bp coords.BufferPoint) coords.SyntaxPoint
func (s SyntaxSnapshot) SyntaxToBuffer(sp coords.SyntaxPoint) coords.BufferPoint
func (s SyntaxSnapshot) SyntaxColWidth(line int) int

type SyntaxMap struct { /* lastBufVer, lastCursorPos */ }

func NewSyntaxMap() SyntaxMap
func (m SyntaxMap) Sync(buf buffer.Buffer, cursors cursor.CursorSet) (SyntaxMap, SyntaxSnapshot)
```

**Phase 1 implementation:** SyntaxMap is a pass-through. All content becomes `TokenText` with `Revealed` state. No markdown parsing. Delta array is empty (no hidden bytes). This makes WrapMap and Snapshot testable immediately.

### `pkg/editor/display/wrap_map.go`

```go
type WrapSegment struct {
    Spans     []SyntaxSpan
    ModelLine int
    WrapIndex int
    StartCol  int
}

type WrapSnapshot struct {
    Segments   []WrapSegment
    TotalRows  int
    // internal lookup tables
}

func (w WrapSnapshot) SyntaxToWrap(sp coords.SyntaxPoint) coords.WrapPoint
func (w WrapSnapshot) WrapToSyntax(wp coords.WrapPoint) coords.SyntaxPoint
func (w WrapSnapshot) ModelLineToFirstRow(line int) int
func (w WrapSnapshot) RowToModelLine(row int) int

type WrapMap struct { /* width int */ }

func NewWrapMap(width int) WrapMap
func (w WrapMap) SetWidth(width int) WrapMap
func (w WrapMap) Sync(ss SyntaxSnapshot) WrapSnapshot
```

**Wrap algorithm:**
- Measure display width using `runewidth.StringWidth`
- If line ≤ width → one segment
- If line > width → break at word boundaries, fallback at exact column
- Never break inside multi-byte rune
- Tab expansion: `\t` expands to next 4-column tab stop during width measurement

### `pkg/editor/display/snapshot.go`

```go
type StyledSpan struct {
    Text  string
    Style lipgloss.Style
}

type DisplayLine struct {
    Spans     []StyledSpan
    ModelLine int
    WrapIndex int
}

type DisplaySnapshot struct {
    Lines     []DisplayLine
    TotalRows int
}

func BuildSnapshot(ws WrapSnapshot, styleCfg StyleConfig) DisplaySnapshot
func (ds DisplaySnapshot) Slice(topRow, height int) []DisplayLine
func (ds DisplaySnapshot) SliceH(lines []DisplayLine, scrollCol, width int) []DisplayLine
```

### `pkg/editor/display/display_test.go`

- Pass-through SyntaxMap: content in = content out (no deltas)
- Coordinate round-trips: `SyntaxToWrap(WrapToSyntax(wp)) == wp`
- Soft-wrap at width (single long line → multiple segments)
- Tab expansion width calculation
- Slice: correct viewport extraction
- SliceH: horizontal truncation when wrap disabled
- Wide characters (CJK) don't break mid-rune

## Constraints

- Domain package: do NOT import `lipgloss` directly. Use a `StyleConfig` interface/func injected from UI layer for `BuildSnapshot`
- Value semantics throughout
- Under 500 LoC per file (split into 3 files)

## QA Gates

These gates protect WP8 (editor View depends on correct display), WP9 (vertical navigation uses wrap rows), WP14 (markdown preview extends SyntaxMap), and WP16 (mouse clicks use Display→Buffer conversion).

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | **P5a:** Pass-through SyntaxMap: `SyntaxToBuffer(BufferToSyntax(bp)) == bp` for ALL valid buffer offsets | Screen lies about cursor position — user operates on wrong text |
| 2 | **P5b:** `WrapToSyntax(SyntaxToWrap(sp)) == sp` for all cursor-legal syntax positions | Cursor on wrap continuation row maps to wrong buffer position → wrong text edited |
| 3 | Monotonicity: for buffer offsets a < b on same line, `BufferToSyntax(a).Col ≤ BufferToSyntax(b).Col` | Display column ordering violated → selection highlight shows wrong range |
| 4 | Tab at col 0 = 4 cells wide; tab at col 1 = 3 cells; tab at col 4 = 4 cells (position-dependent) | Wrong tab width → cursor drifts rightward on every tab → all subsequent text misaligned |
| 5 | CJK character ('中') occupies 2 cells in wrap width calculation | Line wraps at wrong position → text overflows terminal width or wraps too early |
| 6 | Soft-wrap never breaks inside a multi-byte rune | Broken rune at wrap boundary → display shows garbage characters |
| 7 | `Slice(topRow, height)` returns exactly `min(height, TotalRows-topRow)` lines | Viewport shows wrong section of document → user edits invisible text |

**Testing approach:** P5a/P5b via fuzz (random valid content + positions, assert round-trip). Gates 4-7 via table-driven with specific content.

## Verification

```bash
go test ./pkg/editor/display/ -v
```
