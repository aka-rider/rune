# WP15 — Clipboard Operations

## Scope

Clipboard commands in editor component

## Dependencies

- WP10 (basic editing — clipboard uses same edit infrastructure)

## Deliverables

### 3 Clipboard Commands

| Command | Key | Behavior |
|---------|-----|----------|
| `clipboard.copy` | `Cmd+C` | Copy selection to OS clipboard. No selection → copy entire line (incl. newline). Multi-cursor → concatenate with newlines. |
| `clipboard.cut` | `Cmd+X` | Copy + delete. No selection → cut entire line. One history group. |
| `clipboard.paste` | `Cmd+V` | Two-phase async. Insert clipboard at cursor, replace selection. Multi-cursor N lines to N cursors → distribute. |

### Two-Phase Paste Protocol (spec §G)

**Phase 1 (async):** Command returns `tea.Cmd` that reads OS clipboard:
```go
func pasteCmd() tea.Cmd {
    return func() tea.Msg {
        text, err := clipboard.Read()
        if err != nil { return ClipboardErrorMsg{Err: err} }
        return ClipboardContentMsg{Text: text}
    }
}
```

**Phase 2 (sync):** Editor receives `ClipboardContentMsg`, computes edits, applies via `applyOperation`:
```go
case ClipboardContentMsg:
    edits, newCursors := m.computePasteEdits(msg.Text)
    op := command.Operation{Kind: OperationEditBuffer, Edits: edits, Cursors: newCursors}
    m = m.applyOperation(op, history.EditPaste, time.Now())
```

### Multi-Cursor Paste Distribution

```go
func (m Model) computePasteEdits(text string) ([]buffer.Edit, cursor.CursorSet) {
    lines := strings.Split(text, "\n")
    cursors := m.cursors.All()
    if len(lines) == len(cursors) {
        // Distribute one line per cursor
    } else {
        // Paste full text at each cursor
    }
}
```

### Copy — Immediate (No Buffer Mutation)

```go
// Extract text synchronously
// Single cursor + selection → buf.Slice(start, end)
// Single cursor no selection → buf.Line(cursorLine) + "\n"
// Multi-cursor → join selections with "\n"
// Return tea.Cmd that writes to OS clipboard
```

### Cut — Copy + Delete

```go
// Extract text (same as copy)
// Generate delete edits (same as delete operation)
// Apply edits synchronously via applyOperation (records history)
// Return tea.Cmd to write text to OS clipboard
```

### OS Clipboard Integration

Use an injected clipboard port for tests and editor construction. Do not hard-code `pbcopy`/`pbpaste` calls inside command logic that tests must exercise.

```go
type ClipboardPort struct {
    ReadText  func() (string, error)
    WriteText func(string) error
}
```

Production wiring may use platform tools through this port.

Platform detection for clipboard access:
- macOS: `pbcopy` / `pbpaste`
- Linux: `xclip` or `xsel`
- Fallback: OSC-52 terminal escape sequence
- Internal clipboard as final fallback (no OS integration)

### Messages

```go
type ClipboardContentMsg struct {
    Text      string   // text content (mutually exclusive with ImageData)
    ImageData []byte   // image bytes (if clipboard contains image — used by WP17)
    MIMEType  string   // e.g. "image/png" (set when ImageData is non-nil)
}
type ClipboardErrorMsg struct { Err error }
type ClipboardWrittenMsg struct{}
```

### Tests (from qa-implementation-specs.md)

```go
{"paste/basic", "hel|lo", "XY", "clipboard.paste", "helXY|lo"},
{"paste/replace-sel", "h[ell]o", "XY", "clipboard.paste", "hXY|o"},
{"paste/distribute", "a|a\nb|b", "X\nY", "clipboard.paste", "aX|a\nbY|b"},
{"copy/no-sel", "hel|lo\nworld", "", "clipboard.copy", wantClipboard: "hello\n"},
{"copy/with-sel", "h[ell]o", "", "clipboard.copy", wantClipboard: "ell"},
{"cut/with-sel", "h[ell]o", "", "clipboard.cut", "h|o", wantClipboard: "ell"},
{"cut/no-sel", "hel|lo\nworld", "", "clipboard.cut", "|world", wantClipboard: "hello\n"},
```

## Constraints

- Paste is EditPaste kind (never coalesces)
- Cut creates single history group (copy + delete = one undo)
- Cmd closures capture local values, not model fields (§5.5)
- Tests use mock clipboard (inject clipboard read/write functions)
- WP15 owns text clipboard behavior. It defines image-capable message fields, but WP17 owns image save/render/copy behavior.
- Under 500 LoC per file

## QA Gates

These gates protect clipboard workflow correctness — copy/paste is one of the most frequent user operations.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | Paste N lines into N cursors → line i goes to cursor i (distribute) | User copies 3 lines, places 3 cursors, pastes → all 3 get the same full text instead of one line each |
| 2 | Paste N lines into M cursors (N≠M) → full text pasted at each cursor (no distribution) | Mismatched line/cursor count → some cursors get partial content or nothing |
| 3 | Copy with no selection = entire line including trailing `\n` | User copies a line via Cmd+C (no selection), pastes elsewhere → missing newline → lines concatenate |
| 4 | Cut with selection: content deleted from buffer AND clipboard contains exactly the selected text | Cut leaves content in buffer (not actually cut) or clipboard has wrong text |
| 5 | Paste is EditPaste kind: never coalesces with adjacent typing | Paste coalesces with typing → undo removes paste + recent typing as one unit |
| 6 | Clipboard text with trailing newline preserves distribution semantics exactly | Paste strips or invents newlines, corrupting line-oriented clipboard workflows |

**Testing approach:** Table-driven with mock clipboard. Explicit scenarios for distribution vs. full-paste edge cases.

## Verification

```bash
go test ./pkg/ui/components/editor/ -run TestSpec_Clipboard -v
```
