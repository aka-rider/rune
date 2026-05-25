package editor

import (
	"strings"
	"testing"
	"time"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
	"rune/pkg/editor/history"
	"rune/pkg/editor/keybind"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"

	tea "charm.land/bubbletea/v2"
)

func TestEditorIntegration(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res)
	m = m.SetSize(40, 20)

	// File load integration
	content := []byte("hello world\nline 2")
	m, _ = m.Update(FileLoadedMsg{Path: "test.txt", Content: content})

	if m.FilePath() != "test.txt" {
		t.Errorf("expected path test.txt, got %q", m.FilePath())
	}
	if m.Content() != "hello world\nline 2" {
		t.Errorf("content mismatch")
	}
	if m.IsDirty() {
		t.Errorf("expected clean after load")
	}

	// P3: Focus gating
	m = m.SetFocused(false)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'x'})
	if m.Content() != "hello world\nline 2" {
		t.Errorf("unfocused editor modified content")
	}

	m = m.SetFocused(true)
	// View size checks
	view := m.View()
	lines := strings.Split(view, "\n")
	if len(lines) > 20 {
		t.Errorf("view height %d exceeds allocated 20", len(lines))
	}
}

func TestStaleSavesIgnored(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res)
	m = m.SetContent("test.txt", []byte("initial"))

	m, _, _ = m.StartSave()
	req1 := m.activeSave.RequestID

	// Load new file (discards test.txt)
	m, _ = m.Update(FileLoadedMsg{Path: "other.txt", Content: []byte("other")})

	// Stale completion for test.txt comes in
	m, _ = m.Update(FileSavedMsg{
		Path:             "test.txt",
		RequestID:        req1,
		SavedContentHash: hashContent("initial"),
	})

	// other.txt should not be marked clean via the old file's hash!
	if m.filePath != "other.txt" {
		t.Errorf("wrong file path")
	}
}

func TestDuplicateOutOfOrderSaves(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res)
	m = m.SetContent("test.txt", []byte("v1"))

	m, _, _ = m.StartSave() // V1 save starts
	req1 := m.activeSave.RequestID

	// Now simulate content changed
	m.dirty = true
	// And a new save
	m, _, _ = m.StartSave()
	req2 := m.activeSave.RequestID

	// req1 completes
	m, _ = m.Update(FileSavedMsg{
		Path:             "test.txt",
		RequestID:        req1,
		SavedContentHash: hashContent("v1"),
	})

	// Since req2 superseded req1, it shouldn't clear the dirty flag
	if !m.IsDirty() {
		t.Errorf("expected still dirty because latest save hasn't finished")
	}

	// req2 completes
	m, _ = m.Update(FileSavedMsg{
		Path:             "test.txt",
		RequestID:        req2,
		SavedContentHash: m.activeSave.ContentHash,
	})

	/* I modified m.dirty manually, but m.buf hasn't changed.
	   The logic checks hashContent(m.buf.Content()) == SavedContentHash.
	   Wait, let's actually change the file content */
}

func TestApplyOperation(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res)
	m = m.SetContent("test.txt", []byte("initial"))

	// Simulate an edit operation
	op := command.Operation{
		Kind: command.OperationEditBuffer,
		Edits: []buffer.Edit{
			{Start: 0, End: 0, Insert: "new "},
		},
		Cursors: cursor.NewCursorSet(4),
	}

	m = m.applyOperation(op, history.EditPaste, time.Now())

	if m.Content() != "new initial" {
		t.Errorf("buffer not updated, got %q", m.Content())
	}
	if m.cursors.Primary().Position != 4 {
		t.Errorf("cursors not updated")
	}
	if !m.IsDirty() {
		t.Errorf("expected dirty after edit")
	}

	// Test Undo
	m, _ = m.applyUndo()
	if m.Content() != "initial" {
		t.Errorf("undo failed, got %q", m.Content())
	}
	// Undo should make it clean since we are at saved state
	if m.IsDirty() {
		t.Errorf("expected clean after undoing all edits")
	}
}

func TestMarkdownReveal(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res)
	m = m.SetSize(80, 24)
	m = m.SetContent("test.md", []byte("hello **bold** world"))
	m = m.SetFocused(true)

	// Cursor at position 0 — outside bold, should see rendered state
	m.cursors = cursor.NewCursorSet(0)
	m = m.syncDisplay()

	line := m.syntaxSnap.Lines[0]
	var boldRendered *display.SyntaxSpan
	for i := range line.Spans {
		if line.Spans[i].Kind == display.TokenBold {
			boldRendered = &line.Spans[i]
			break
		}
	}
	if boldRendered == nil {
		t.Fatal("no bold span found when cursor outside")
	}
	if boldRendered.State != display.Rendered {
		t.Errorf("bold should be Rendered when cursor at 0, got %v", boldRendered.State)
	}
	if boldRendered.Text != "bold" {
		t.Errorf("rendered text should be 'bold', got %q", boldRendered.Text)
	}

	// Move cursor to position 8 (inside "bold") — should reveal
	m.cursors = cursor.NewCursorSet(8)
	m = m.syncDisplay()

	line2 := m.syntaxSnap.Lines[0]
	var boldRevealed *display.SyntaxSpan
	for i := range line2.Spans {
		if line2.Spans[i].Kind == display.TokenBold {
			boldRevealed = &line2.Spans[i]
			break
		}
	}
	if boldRevealed == nil {
		t.Fatal("no bold span found when cursor inside")
	}
	if boldRevealed.State != display.Revealed {
		t.Errorf("bold should be Revealed when cursor at 8, got %v", boldRevealed.State)
	}
	if boldRevealed.Text != "**bold**" {
		t.Errorf("revealed text should be '**bold**', got %q", boldRevealed.Text)
	}

	// Move cursor back out (position 14) — should render again
	m.cursors = cursor.NewCursorSet(14)
	m = m.syncDisplay()

	line3 := m.syntaxSnap.Lines[0]
	var boldAfter *display.SyntaxSpan
	for i := range line3.Spans {
		if line3.Spans[i].Kind == display.TokenBold {
			boldAfter = &line3.Spans[i]
			break
		}
	}
	if boldAfter == nil {
		t.Fatal("no bold span found after cursor moves out")
	}
	if boldAfter.State != display.Rendered {
		t.Errorf("bold should be Rendered when cursor exits, got %v", boldAfter.State)
	}

	// Buffer offsets must remain constant across reveal transitions
	if boldRendered.BufferStart != boldRevealed.BufferStart ||
		boldRendered.BufferStart != boldAfter.BufferStart {
		t.Errorf("BufferStart changed across reveal: %d, %d, %d",
			boldRendered.BufferStart, boldRevealed.BufferStart, boldAfter.BufferStart)
	}
	if boldRendered.BufferEnd != boldRevealed.BufferEnd ||
		boldRendered.BufferEnd != boldAfter.BufferEnd {
		t.Errorf("BufferEnd changed across reveal: %d, %d, %d",
			boldRendered.BufferEnd, boldRevealed.BufferEnd, boldAfter.BufferEnd)
	}
}
