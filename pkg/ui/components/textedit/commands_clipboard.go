package textedit

import (
	"strings"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

// clipboardWriteCmd returns a tea.Cmd that writes text to the system clipboard
// via OSC 52 (Bubble Tea native). Always non-nil for non-empty text.
func clipboardWriteCmd(text string) tea.Cmd {
	if text == "" {
		return nil
	}
	return tea.SetClipboard(text)
}

// clipboardReadCmd returns a tea.Cmd that reads from the system clipboard
// via OSC 52 (Bubble Tea native). The response arrives as tea.ClipboardMsg.
func clipboardReadCmd() tea.Cmd {
	return func() tea.Msg { return tea.ReadClipboard() }
}

var clipboardSpecs = []cmdSpec{
	{name: "clipboard.copy", category: "clipboard", title: "Copy", when: "editorFocused", exec: clipboardCopy},
	{name: "clipboard.cut", category: "clipboard", title: "Cut", when: "editorFocused && !readOnly", exec: clipboardCut},
	{name: "clipboard.paste", category: "clipboard", title: "Paste", when: "editorFocused && !readOnly", exec: clipboardPaste},
}

func clipboardCopy(ctx command.CommandContext) command.Result {
	text := extractCopyText(ctx.Buffer, ctx.Cursors)
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationNone,
			Cursors: ctx.Cursors,
		},
		Cmd: clipboardWriteCmd(text),
	}
}

func clipboardCut(ctx command.CommandContext) command.Result {
	text := extractCopyText(ctx.Buffer, ctx.Cursors)
	edits, newCursors := buildDeleteEdits(ctx.Buffer, ctx.Cursors)

	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationEditBuffer,
			Edits:   edits,
			Cursors: cursor.NewCursorSetFrom(newCursors),
		},
		Cmd: clipboardWriteCmd(text),
	}
}

func clipboardPaste(ctx command.CommandContext) command.Result {
	// Phase 1: return a Cmd that reads from clipboard via OSC 52.
	// The editor's Update will handle tea.ClipboardMsg (phase 2).
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationNone,
			Cursors: ctx.Cursors,
		},
		Cmd: clipboardReadCmd(),
	}
}

// extractCopyText builds the text to copy based on cursor state.
func extractCopyText(buf buffer.Buffer, cursors cursor.CursorSet) string {
	all := cursors.All()
	if len(all) == 0 {
		return ""
	}

	if len(all) == 1 {
		c := all[0]
		if c.HasSelection() {
			start, end := c.SelectionStart(), selectionEndInclusive(c, buf)
			return buf.Slice(start, end)
		}
		// No selection: copy entire line including newline
		return copyEntireLine(buf, c.Position)
	}

	// Multi-cursor: join selections/lines with newline
	var parts []string
	for _, c := range all {
		if c.HasSelection() {
			start, end := c.SelectionStart(), selectionEndInclusive(c, buf)
			parts = append(parts, buf.Slice(start, end))
		} else {
			parts = append(parts, copyEntireLine(buf, c.Position))
		}
	}
	return strings.Join(parts, "\n")
}

// copyEntireLine returns the full line at offset, including trailing newline if present.
func copyEntireLine(buf buffer.Buffer, offset int) string {
	bp := buf.OffsetToLineCol(offset)
	lineStart := buf.LineStart(bp.Line)
	lineEnd := buf.LineEnd(bp.Line)
	// Include trailing newline if not last line
	if lineEnd < buf.Len() {
		lineEnd++ // include the \n
	}
	return buf.Slice(lineStart, lineEnd)
}

// buildDeleteEdits creates edits to delete selections (or lines) for cut.
func buildDeleteEdits(buf buffer.Buffer, cursors cursor.CursorSet) ([]buffer.Edit, []cursor.Cursor) {
	all := cursors.All()

	var infos []editInfoItem
	for _, c := range all {
		if c.HasSelection() {
			start, end := c.SelectionStart(), selectionEndInclusive(c, buf)
			infos = append(infos, editInfoItem{edit: buffer.Edit{Start: start, End: end, Insert: "", CursorID: c.ID}, cID: c.ID})
		} else {
			// Delete entire line — line-oriented, not selection-exact, so left
			// untagged (CursorID 0); SEL-EDIT only checks selecting cursors anyway.
			bp := buf.OffsetToLineCol(c.Position)
			lineStart := buf.LineStart(bp.Line)
			lineEnd := buf.LineEnd(bp.Line)
			if lineEnd < buf.Len() {
				lineEnd++ // include the \n
			}
			infos = append(infos, editInfoItem{edit: buffer.Edit{Start: lineStart, End: lineEnd, Insert: ""}, cID: c.ID})
		}
	}

	sortInfosDescending(infos)
	edits := infosToEdits(infos)
	newCursors := computePostEditCursors(infos)

	return edits, newCursors
}

// handlePasteContent is phase 2: apply the clipboard text as edits. Read-only
// content (e.g. the Help view) must never be mutated by ANY input path —
// edit.insert-character's ResultNoMatch branch already guards read-only for
// keyboard characters (textedit.go), but paste/clipboard bypassed it
// entirely: pressing F1 then pasting silently mutated the "read-only" Help
// buffer (found via FuzzSaveRace — a read-only doc getting Paste'd into
// produced a reversed selection whose Selected cells then tripped S1, but
// the mutation itself, independent of that checker finding, is the real
// bug — a read-only view is supposed to mean read-only). Guarded HERE, the
// single function all three callers (ClipboardContentMsg, tea.ClipboardMsg,
// tea.PasteMsg) funnel through, rather than patching each call site.
func (m Model) handlePasteContent(text string) (Model, tea.Cmd) {
	if text == "" || m.readOnly {
		return m, nil
	}

	all := m.cursors.All()
	if len(all) == 0 {
		return m, nil
	}

	lines := strings.Split(text, "\n")
	// Strip trailing empty element from trailing newline for distribution check.
	if len(lines) > 1 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	distribute := len(lines) == len(all) && len(all) > 1

	res := perCursorSelectionEdits(command.CommandContext{Buffer: m.buf, Cursors: m.cursors},
		func(i int, c cursor.Cursor) string {
			if distribute {
				return lines[i]
			}
			return text
		},
		func(c cursor.Cursor) (int, int, bool) { return c.Position, c.Position, true })

	m = m.applyResult(res)
	return m, nil
}
