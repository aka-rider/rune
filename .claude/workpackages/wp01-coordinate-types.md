# WP1 — Coordinate Types

## Scope

`pkg/editor/coords/coords.go`

## Dependencies

None (can run in parallel with WP0, WP5, WP6)

## Deliverables

### `pkg/editor/coords/coords.go`

Pure type declarations — no logic beyond constructors. These are named types that prevent accidentally passing a buffer offset where a display column is expected.

```go
package coords

// Buffer Space — raw byte positions in the UTF-8 document
type BufferOffset int
type BufferPoint struct {
    Line int  // 0-indexed model line number
    Col  int  // byte offset from start of that line
}

// Syntax Space — positions after markdown tokens are folded/expanded
type SyntaxOffset int
type SyntaxPoint struct {
    Line int  // same model line as buffer (1:1 line mapping)
    Col  int  // column in syntax space
}

// Wrap Space — positions after soft-wrap breaks are inserted
type WrapRow int
type WrapPoint struct {
    Row int  // display row (0-indexed from document top)
    Col int  // column within that wrapped segment
}

// Display Space — final terminal grid after viewport slicing
type DisplayRow int
type DisplayPoint struct {
    Row int  // row relative to viewport top (0 = first visible line)
    Col int  // column (includes tab expansion)
}
```

## Constraints

- No imports from other `pkg/editor/` packages
- No logic — pure data types
- Under 500 LoC

## Verification

```bash
go build ./pkg/editor/coords/...
```

Compiles successfully. Types are consumed by WP2, WP3, WP7.
