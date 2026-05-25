package editor

import (
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/history"
)

func registerClipboardCommands(builder command.Builder) (command.Builder, error) {
	var err error

	builder, err = builder.Register(command.Command{
		Name:     "clipboard.copy",
		Category: "clipboard",
		Title:    "Copy",
		Execute:  clipboardCopy,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:     "clipboard.cut",
		Category: "clipboard",
		Title:    "Cut",
		Execute:  clipboardCut,
	})
	if err != nil {
		return builder, err
	}

	builder, err = builder.Register(command.Command{
		Name:     "clipboard.paste",
		Category: "clipboard",
		Title:    "Paste",
		Execute:  clipboardPaste,
	})
	if err != nil {
		return builder, err
	}

	return builder, nil
}

func clipboardCopy(ctx command.CommandContext) command.Result {
	text := extractCopyText(ctx.Buffer, ctx.Cursors)
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationClipboard,
			Cursors: ctx.Cursors,
		},
		Cmd: writeClipboardCmd(text),
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
		Cmd: writeClipboardCmd(text),
	}
}

func clipboardPaste(ctx command.CommandContext) command.Result {
	// Phase 1: return a Cmd that reads from clipboard.
	// The editor's Update will handle ClipboardContentMsg (phase 2).
	return command.Result{
		Operation: command.Operation{
			Kind:    command.OperationClipboard,
			Cursors: ctx.Cursors,
		},
		Cmd: readClipboardCmd(),
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
			start, end := c.SelectionRange()
			return buf.Slice(start, end)
		}
		// No selection: copy entire line including newline
		return copyEntireLine(buf, c.Position)
	}

	// Multi-cursor: join selections/lines with newline
	var parts []string
	for _, c := range all {
		if c.HasSelection() {
			start, end := c.SelectionRange()
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

	type cutEditInfo struct {
		edit buffer.Edit
		cID  int
	}

	var infos []cutEditInfo
	for _, c := range all {
		if c.HasSelection() {
			start, end := c.SelectionRange()
			infos = append(infos, cutEditInfo{
				edit: buffer.Edit{Start: start, End: end, Insert: ""},
				cID:  c.ID,
			})
		} else {
			// Delete entire line
			bp := buf.OffsetToLineCol(c.Position)
			lineStart := buf.LineStart(bp.Line)
			lineEnd := buf.LineEnd(bp.Line)
			if lineEnd < buf.Len() {
				lineEnd++ // include the \n
			}
			infos = append(infos, cutEditInfo{
				edit: buffer.Edit{Start: lineStart, End: lineEnd, Insert: ""},
				cID:  c.ID,
			})
		}
	}

	// Sort descending by start
	for i := 0; i < len(infos)-1; i++ {
		for j := i + 1; j < len(infos); j++ {
			if infos[i].edit.Start < infos[j].edit.Start {
				infos[i], infos[j] = infos[j], infos[i]
			}
		}
	}

	edits := make([]buffer.Edit, len(infos))
	for i, info := range infos {
		edits[i] = info.edit
	}

	// Compute post-edit cursor positions
	var newCursors []cursor.Cursor
	shift := 0
	for i := len(infos) - 1; i >= 0; i-- {
		info := infos[i]
		newPos := info.edit.Start + shift
		newCursors = append(newCursors, cursor.Cursor{
			Position: newPos,
			Anchor:   newPos,
			ID:       info.cID,
		})
		shift += len(info.edit.Insert) - (info.edit.End - info.edit.Start)
	}

	return edits, newCursors
}

// handlePasteContent is phase 2: apply the clipboard text as edits.
func (m Model) handlePasteContent(text string, now time.Time) (Model, tea.Cmd) {
	if text == "" {
		return m, nil
	}

	all := m.cursors.All()
	if len(all) == 0 {
		return m, nil
	}

	lines := strings.Split(text, "\n")
	// Strip trailing empty element from trailing newline for distribution check.
	// e.g., "X\nY\n" → ["X","Y",""] → treat as ["X","Y"] for N-cursor distribution.
	if len(lines) > 1 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	distribute := len(lines) == len(all) && len(all) > 1

	type editInfo struct {
		edit buffer.Edit
		cID  int
	}

	var infos []editInfo
	for i, c := range all {
		var insertText string
		if distribute {
			insertText = lines[i]
		} else {
			insertText = text
		}

		if c.HasSelection() {
			start, end := c.SelectionRange()
			infos = append(infos, editInfo{
				edit: buffer.Edit{Start: start, End: end, Insert: insertText},
				cID:  c.ID,
			})
		} else {
			infos = append(infos, editInfo{
				edit: buffer.Edit{Start: c.Position, End: c.Position, Insert: insertText},
				cID:  c.ID,
			})
		}
	}

	// Sort descending
	for i := 0; i < len(infos)-1; i++ {
		for j := i + 1; j < len(infos); j++ {
			if infos[i].edit.Start < infos[j].edit.Start {
				infos[i], infos[j] = infos[j], infos[i]
			}
		}
	}

	edits := make([]buffer.Edit, len(infos))
	for i, info := range infos {
		edits[i] = info.edit
	}

	// Compute post-edit cursors (cursor at end of inserted text)
	var newCursors []cursor.Cursor
	shift := 0
	for i := len(infos) - 1; i >= 0; i-- {
		info := infos[i]
		insLen := len(info.edit.Insert)
		newPos := info.edit.Start + shift + insLen
		newCursors = append(newCursors, cursor.Cursor{
			Position: newPos,
			Anchor:   newPos,
			ID:       info.cID,
		})
		shift += insLen - (info.edit.End - info.edit.Start)
	}

	op := command.Operation{
		Kind:    command.OperationEditBuffer,
		Edits:   edits,
		Cursors: cursor.NewCursorSetFrom(newCursors),
	}

	m = m.applyOperation(op, history.EditPaste, now)
	m = m.syncDisplay()
	return m, nil
}

// readClipboardCmd returns a tea.Cmd that reads from the clipboard.
// The clipboard port is accessed via a package-level variable set during dispatch.
// Instead, we use a closure pattern — the editor sets this up in dispatchOperation.
func readClipboardCmd() tea.Cmd {
	return nil // placeholder; actual cmd is built in dispatchClipboardRead
}

func writeClipboardCmd(text string) tea.Cmd {
	// placeholder; actual cmd is built in dispatchClipboardWrite
	_ = text
	return nil
}

// buildReadClipboardCmd creates a tea.Cmd that reads from the clipboard port.
func buildReadClipboardCmd(port ClipboardPort) tea.Cmd {
	if port.ReadText == nil {
		return nil
	}
	readFn := port.ReadText
	return func() tea.Msg {
		text, err := readFn()
		if err != nil {
			return ClipboardErrorMsg{Err: err}
		}
		return ClipboardContentMsg{Text: text}
	}
}

// buildWriteClipboardCmd creates a tea.Cmd that writes text to the clipboard port.
func buildWriteClipboardCmd(port ClipboardPort, text string) tea.Cmd {
	if port.WriteText == nil {
		return nil
	}
	writeFn := port.WriteText
	capturedText := text
	return func() tea.Msg {
		err := writeFn(capturedText)
		if err != nil {
			return ClipboardErrorMsg{Err: err}
		}
		return ClipboardWrittenMsg{}
	}
}
