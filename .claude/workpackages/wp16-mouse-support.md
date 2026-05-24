# WP16 — Mouse Support

## Scope

Mouse actions in editor + terminal setup in `cmd/rune/main.go`

## Dependencies

- WP8 (editor component with display pipeline for coordinate conversion)

## Deliverables

### Terminal Setup

In `cmd/rune/main.go`, enable Bubble Tea mouse support:
```go
p := tea.NewProgram(model, tea.WithAltScreen(), tea.WithMouseAllMotion())
```

### 7 Mouse Actions

| Action | Trigger | Behavior |
|--------|---------|----------|
| `mouse.click` | Single click | Position cursor at clicked display coordinate (DisplayToModel conversion). Clear selection and secondary cursors. |
| `mouse.click-shift` | Shift+click | Extend selection from current anchor to clicked position. |
| `mouse.click-alt` | Alt+click | Add new cursor at clicked position (multi-cursor via mouse). |
| `mouse.double-click` | Double click | Select word under click position. |
| `mouse.triple-click` | Triple click | Select entire line under click position. |
| `mouse.drag` | Click+drag | Start selection from mousedown, extend to current position on move. |
| `mouse.scroll-up/down` | Scroll wheel | Scroll viewport by 3 lines (configurable). Cursor does not move. |

### Coordinate Conversion (DisplayToModel)

```go
func displayToBuffer(dp coords.DisplayPoint, v ViewportState, ws display.WrapSnapshot, ss display.SyntaxSnapshot) coords.BufferPoint {
    wp := v.DisplayToWrap(dp)
    sp := ws.WrapToSyntax(wp)
    return ss.SyntaxToBuffer(sp)
}
```

Then convert `BufferPoint` to byte offset via `buf.LineColToOffset(bp)`.

### Double-Click Word Selection

1. Convert click position to buffer offset
2. Find word boundaries around that offset (same rules as `cursor.word-left`/`cursor.word-right`)
3. Set Anchor = word start, Position = word end

### Triple-Click Line Selection

1. Convert click position to buffer line
2. Set Anchor = LineStart(line), Position = LineStart(line+1) or LineEnd(line) for last line

### Drag Selection

Track mousedown position as anchor. On every mouse move event, update Position to current mouse coordinate (converted to buffer offset).

### Mouse State Tracking

Need to track:
- Last click time (for double/triple click detection, threshold ~500ms)
- Last click position (must be same position for multi-click)
- Mouse button state (for drag detection)
- Click count (1, 2, 3)

### Tests

```go
// Click positions cursor correctly
{"click/basic", width=40, click at (5, 2), expect cursor at line 2 col 5},

// Click with soft-wrap
{"click/wrapped", long line wrapped, click on second display row, correct buffer offset},

// Shift+click extends selection
{"shift-click", cursor at (0,0), shift-click at (5,0), selection [0,5]},

// Alt+click adds cursor
{"alt-click", cursor at (0,0), alt-click at (5,2), two cursors},

// Double-click selects word
{"double-click", "hello world", double-click on 'o', selection "hello"},

// Scroll doesn't move cursor
{"scroll", cursor at line 5, scroll up 3, cursor still line 5, viewport shifted},
```

## Constraints

- Mouse events only processed when editor is focused
- Coordinate conversion uses full pipeline (Display → Wrap → Syntax → Buffer)
- Scroll lines configurable (default 3)
- Under 500 LoC per file

## QA Gates

These gates ensure mouse clicks land on the correct buffer position — critical for trust in point-and-click editing.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | Click at display position (row, col) with TopRow offset → cursor at correct buffer byte offset (accounts for scroll + wrap + folding) | User clicks on a word but cursor lands on different word → edits wrong text |
| 2 | Double-click selects word (anchor at word-start, position at word-end) matching `word-left`/`word-right` boundaries | Double-click selects wrong range → user’s word operation is wrong |
| 3 | Triple-click selects entire line (including newline, matching "copy line" semantics) | Triple-click misses characters → line selection is incomplete |
| 4 | Shift+click extends selection from current anchor to clicked position | Shift+click starts new selection instead of extending → user loses existing selection |
| 5 | Scroll wheel does not move cursor position (only viewport moves) | Scroll accidentally repositions cursor → user’s next edit goes to wrong place |

**Testing approach:** Table-driven with known viewport state + coordinates. Each test constructs specific display geometry and asserts buffer offset.

## Verification

```bash
go build ./cmd/rune/...
go test ./pkg/ui/components/editor/ -run TestMouse -v
```

Manual: run app, click in editor, verify cursor placement, drag select, scroll.
