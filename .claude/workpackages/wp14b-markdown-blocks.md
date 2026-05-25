# WP14B — Markdown Block Preview

## Scope

Extend `SyntaxMap` live preview for block-level markdown elements that affect multiple lines:

- code fences
- tables

These features are more fragile than inline folding because they affect row counts, wrapping, and block reveal rules.

## Dependencies

- WP14A (parser selected and inline/line delta logic proven)
- WP7 (WrapMap and DisplaySnapshot)
- WP8 (scroll preservation hooks)

## Deliverables

### Code Fences

- Detect opening/closing fences with byte-accurate source ranges.
- Rendered state hides fence markers and exposes semantic spans for code content plus language metadata.
- Revealed state shows the full raw fence block when any cursor is inside the block line range.
- Do not add syntax highlighting in `pkg/editor/display`; leave UI styling/highlighting to the editor renderer or a later package.

Exact line-oriented span contract consumed by WP14D. A multi-line fence produces one or more spans per visible line/segment; it must not produce one giant span containing embedded newlines.

```go
display.DisplaySpan{
	Kind:        display.TokenCodeFence,
	State:       display.Rendered, // or Revealed when cursor is inside block
	Text:        codeLineOrRawFenceLine,
	Language:    fenceLanguage,    // empty when not declared
	BufferStart: lineOrSegmentStartOffset,
	BufferEnd:   lineOrSegmentEndOffset,
	BlockID:     stableFenceBlockID,
	BlockStart:  fenceStartOffset,
	BlockEnd:    fenceEndOffset,
}
```

WP14B tests must produce this shape from a real multi-line fenced block fixture through SyntaxMap/WrapMap/DisplaySnapshot construction; WP14D must consume a WP14B-produced fixture, not a hand-built UI-only substitute.

### Tables

- Detect contiguous markdown table blocks with source ranges.
- Rendered state emits semantic table spans/rows that preserve Buffer-to-Syntax coordinate mappings.
- Revealed state shows raw table source when cursor is inside the table block.
- If parser table source ranges are insufficient, implement table preview as raw-only for this package and document the blocker.

### Scroll Stability

- If block reveal/hide changes `TotalRows`, preserve the cursor's model-line anchor relative to viewport top.
- Use existing `scrollPreservingAnchor` behavior from WP8; do not duplicate viewport math in `pkg/editor/display`.

## Non-Goals

- Math blocks, callouts, frontmatter, embeds, image placeholders.
- Terminal image rendering.
- UI syntax highlighting.

## Tests

- Code fence rendered vs revealed state.
- Cursor on opening fence, body line, and closing fence all reveal the full block.
- Cursor outside the block renders the block.
- Table rendered vs revealed state with header separator.
- Coordinate round-trips at block boundaries.
- Scroll preservation when code fence reveal changes row count.

## QA Gates

| # | Gate | Harm Prevented |
|---|---|---|
| 1 | Cursor anywhere in a fence reveals the whole fence | User edits hidden fence delimiters without seeing syntax |
| 2 | Block reveal does not corrupt coordinate round-trips at first/last block line | Clicks/selections land on wrong bytes near block edges |
| 3 | Table fallback is explicit if source ranges are not trustworthy | Worker invents fragile string parsing that breaks documents |
| 4 | Row-count changes preserve viewport anchor | Cursor entering a block causes disorienting jump |

## Verification

```bash
go test ./pkg/editor/display/ -run 'TestCodeFence|TestMarkdownTable|TestSyntaxMapBlock' -v
go test ./pkg/ui/components/editor/ -run TestScrollPreservation -v
```
