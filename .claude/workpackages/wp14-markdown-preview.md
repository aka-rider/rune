# WP14 — Markdown Live Preview (SyntaxMap Full Implementation)

## Scope

`pkg/editor/display/syntax_map.go` — upgrade from pass-through to full markdown parsing with reveal/hide

## Dependencies

- WP7 (display pipeline — pass-through SyntaxMap)
- WP8 (editor component — for integration)

## Deliverables

### Markdown Parser Integration

Choose parser with byte-accurate source ranges (goldmark with AST source positions, or similar).

Add dependency to `go.mod`.

### Reveal Granularity Rules (from spec)

| Granularity | Elements | Trigger |
|-------------|----------|---------|
| Per-token (inline) | Bold, italic, strikethrough, inline code, links, inline math, highlights | Cursor byte-offset within token's source span [start, end) |
| Per-line | Headings, horizontal rules, task lists, blockquote markers | Cursor anywhere on that model line |
| Per-block | Code fences, tables, math blocks, callouts | Cursor anywhere within block's line range |
| Always visible | List markers, tags, raw URLs | Never hidden; styled but syntax shown |

### Cursor Proximity Detection

Given cursor at byte offset P and AST node with source range [start, end):
1. Token reveal: `start <= P < end` for inline node → raw syntax shown
2. Line reveal: P on same model line as line-level element → raw syntax shown
3. Block reveal: P within block's line range [firstLine, lastLine] → entire block raw

Nested: cursor inside bold within blockquote → bold reveals but blockquote doesn't (unless cursor on `>` line)

### Element Rendering Table

Full implementation for:
- Headings (`#` hidden when rendered, large/bold style)
- Bold (`**` hidden, bold style)
- Italic (`*`/`_` hidden, italic style)
- Strikethrough (`~~` hidden, strikethrough style)
- Inline code (backticks hidden, monospace+background)
- Links (`[text](url)` → styled text only, brackets+url hidden)
- Blockquotes (`>` hidden, left border + indent)
- Task lists (`- [ ]`/`- [x]` → checkbox glyph)
- Horizontal rules (`---` → visual divider)
- Code fences (fence markers hidden, syntax highlighting with background)
- Tables (aligned grid with box-drawing)
- Math blocks (styled raw as fallback)
- Inline math (styled raw as fallback)
- Frontmatter (collapsed indicator or key-value table)
- Callouts (styled box with icon)
- Images (placeholder — terminal rendering in WP17)
- Highlights (`==text==` — `==` hidden, highlight background style)
- Embed references (per-line reveal, same as headings)
- Frontmatter (configurable: visible/hidden/source — expose config option)

### Offset Delta Tracking

For each rendered span where delimiters are hidden:
- Track cumulative bytes hidden in `[]OffsetDelta`
- Enable O(log N) bidirectional coordinate conversion via binary search
- Example from spec: `"## Hello **world** end"` → deltas at offsets 0, 10, 17

### Style Mapping

Styles applied in UI layer, not domain package:
- `StyleConfig` (function or struct) passed to `BuildSnapshot`
- Maps `(TokenKind, RevealState)` → `lipgloss.Style`
- Domain package only produces `SyntaxSpan` with kind+state

### Tests

```go
// Offset delta accuracy
{"heading-rendered", "## Hello", cursorAt(20), wantSyntaxCol("Hello", 0)},
{"bold-rendered", "Some **bold** end", cursorAt(0), wantHidden("**", 4)},
{"bold-revealed", "Some **bold** end", cursorAt(7), wantVisible("**bold**")},

// Coordinate round-trips with folding
{"round-trip-cursor-legal", bp, BufferToSyntax→SyntaxToBuffer == identity},

// Reveal transitions
{"enter-token", moveCursorInto("**bold**"), stateBecomesRevealed},
{"exit-token", moveCursorOut("**bold**"), stateBecomesRendered},
```

Golden tests:
- `heading-away`: cursor in body → heading rendered (# hidden)
- `heading-on`: cursor on heading → raw shown
- `bold-away`: bold rendered (** hidden)
- `bold-on`: cursor inside → ** visible
- `code-fence`: fence markers hidden when cursor outside
- `soft-wrap`: wraps correctly with folded content

Scroll stability test:
- Cursor enters `**bold**` token → line width increases → TotalRows may change → viewport TopRow adjusts so cursor stays at same screen Y position (no visual jump)

## Constraints

- Parser MUST expose byte-accurate source ranges
- Domain package (`pkg/editor/display/`) must NOT import `lipgloss`
- Coordinate conversion must remain correct with folded content
- Under 500 LoC per file (split syntax_map if needed)

## QA Gates

These gates protect visual correctness when markdown tokens are hidden/shown. A failure here means the screen lies about cursor position.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | Coordinate round-trip with hidden delimiters: `BufferToSyntax(SyntaxToBuffer(sp)) == sp` for all cursor-legal positions (not inside hidden tokens) | Cursor appears at wrong position relative to visible text → user edits wrong character |
| 2 | Cursor entering hidden-delimiter token → delimiters reveal → cursor Position unchanged (same buffer byte offset, display position shifts to accommodate revealed chars) | Cursor jumps to unexpected position when approaching bold/italic text |
| 3 | Scroll stability: reveal/hide transition does not change viewport TopRow relative to cursor | User’s view suddenly scrolls when cursor enters/exits a token → disorienting |
| 4 | Offset deltas are monotonically increasing: `Deltas[i].BufferOffset < Deltas[i+1].BufferOffset` | Binary search in coordinate conversion produces wrong result → display corruption |
| 5 | Selection spanning hidden tokens: byte range [start, end) correctly maps to visual highlight (highlight includes hidden bytes’ visual positions) | User selects across bold text, visual highlight is wrong → copy/delete affects wrong range |

**Testing approach:** Gates 1-2 via property test (random markdown content, random cursor positions, assert round-trips). Gates 3-5 via targeted scenarios. Golden tests for visual output.

## Verification

```bash
go test ./pkg/editor/display/ -run TestSyntaxMap -v
go test ./pkg/ui/components/editor/ -run TestGolden_MarkdownRendering -v -update  # create goldens
go test ./pkg/ui/components/editor/ -run TestGolden_MarkdownRendering -v  # compare
```
