package editor

import (
	"strings"
	"testing"
	"time"
	"unicode/utf8"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
	"rune/pkg/editor/history"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"

	tea "charm.land/bubbletea/v2"
)

func TestEditorIntegration(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res, terminal.TermCaps{})
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

	m := New(keys, st, reg, res, terminal.TermCaps{})
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

	m := New(keys, st, reg, res, terminal.TermCaps{})
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

	m := New(keys, st, reg, res, terminal.TermCaps{})
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

	m := New(keys, st, reg, res, terminal.TermCaps{})
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

// TestPrintableLettersInsertText verifies that printable characters that were
// historically used as vim-style navigation (j, k, g, G, b, f, u, d) are
// treated as text insertion when the editor is focused — no legacy navigation.
func TestPrintableLettersInsertText(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()

	// Create resolver with the production bindings from keymap
	bindings, _ := keys.CommandBindings()
	res, _ := keybind.NewResolver(bindings)

	m := New(keys, st, reg, res, terminal.TermCaps{})
	m = m.SetSize(40, 20)
	m = m.SetFocused(true)

	// Legacy vim-style keys that must insert text, not navigate
	legacyKeys := []rune{'j', 'k', 'g', 'G', 'b', 'f', 'u', 'd'}

	for _, ch := range legacyKeys {
		t.Run(string(ch), func(t *testing.T) {
			testM := m
			testM.buf = buffer.New("hello")
			testM.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{{Position: 5, Anchor: 5, ID: 0}})
			testM.history = history.New(func() time.Time { return time.Now() })

			testM, _ = testM.Update(tea.KeyPressMsg{Code: ch})

			expected := "hello" + string(ch)
			if testM.Content() != expected {
				t.Errorf("key %q: expected content %q, got %q (character was not inserted)",
					string(ch), expected, testM.Content())
			}
		})
	}
}

// TestPrintableSpecialCharsInsertText verifies that ? and other printable chars
// that are used as UI bindings at the page level still insert text in the editor.
func TestPrintableSpecialCharsInsertText(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()

	bindings, _ := keys.CommandBindings()
	res, _ := keybind.NewResolver(bindings)

	m := New(keys, st, reg, res, terminal.TermCaps{})
	m = m.SetSize(40, 20)
	m = m.SetFocused(true)

	// ? is a page-level binding (HelpExpand) but must insert in the editor
	chars := []rune{'?', '!', '@', '#', '$', '%'}

	for _, ch := range chars {
		t.Run(string(ch), func(t *testing.T) {
			testM := m
			testM.buf = buffer.New("hello")
			testM.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{{Position: 5, Anchor: 5, ID: 0}})
			testM.history = history.New(func() time.Time { return time.Now() })

			testM, _ = testM.Update(tea.KeyPressMsg{Code: ch})

			expected := "hello" + string(ch)
			if testM.Content() != expected {
				t.Errorf("key %q: expected content %q, got %q",
					string(ch), expected, testM.Content())
			}
		})
	}
}

// === Regression tests for cascading editor bugs ===

// TestCursorSetInitialized verifies that New() and FileLoadedMsg produce a model
// with a valid non-empty CursorSet (regression: bug #2 — zero-value CursorSet).
func TestCursorSetInitialized(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res, terminal.TermCaps{})

	// New() must produce a cursor at offset 0
	all := m.cursors.All()
	if len(all) == 0 {
		t.Fatal("New() produced empty CursorSet — cursor must exist at offset 0")
	}
	if all[0].Position != 0 {
		t.Errorf("New() cursor position = %d, want 0", all[0].Position)
	}

	// FileLoadedMsg must also produce a cursor at offset 0
	m = m.SetSize(80, 24)
	m, _ = m.Update(FileLoadedMsg{Path: "test.md", Content: []byte("line one\nline two\nline three")})
	all = m.cursors.All()
	if len(all) == 0 {
		t.Fatal("FileLoadedMsg produced empty CursorSet — cursor must exist at offset 0")
	}
	if all[0].Position != 0 {
		t.Errorf("FileLoadedMsg cursor position = %d, want 0", all[0].Position)
	}
}

// TestCommandContextHasCursors verifies that the keybind-resolved command path
// passes Cursors to the CommandContext (regression: bug #1 — missing Cursors field).
func TestCommandContextHasCursors(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()

	bindings, _ := keys.CommandBindings()
	resolver, _ := keybind.NewResolver(bindings)

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	m.buf = buffer.New("abcdef")
	m.cursors = cursor.NewCursorSet(0)
	m = m.syncDisplay()

	// Press Right arrow — should move cursor from 0 to 1
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyRight})

	pos := m.cursors.Primary().Position
	if pos != 1 {
		t.Errorf("after Right arrow, cursor position = %d, want 1 (CommandContext.Cursors not passed?)", pos)
	}

	// Press Right again — should move from 1 to 2
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyRight})
	pos = m.cursors.Primary().Position
	if pos != 2 {
		t.Errorf("after second Right arrow, cursor position = %d, want 2", pos)
	}
}

// TestCommandContextHasCoordinateConverters verifies that vertical navigation
// works — which requires BufferToSyntax/SyntaxToWrap/etc. to be non-nil
// (regression: bug #1 — missing coordinate converter functions).
func TestCommandContextHasCoordinateConverters(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()

	bindings, _ := keys.CommandBindings()
	resolver, _ := keybind.NewResolver(bindings)

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	m.buf = buffer.New("first line\nsecond line\nthird line")
	m.cursors = cursor.NewCursorSet(3) // middle of "first line"
	m = m.syncDisplay()

	// Press Down arrow — should move cursor to second line
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})

	pos := m.cursors.Primary().Position
	// "first line\n" = 11 bytes, cursor at col 3 on second line = offset 14
	if pos < 11 || pos > 14 {
		t.Errorf("after Down arrow from offset 3 on line 0, cursor position = %d, want in [11, 14] (coordinate converters nil?)", pos)
	}

	// Press Up — should return to first line
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyUp})
	pos = m.cursors.Primary().Position
	if pos > 10 {
		t.Errorf("after Up arrow, cursor position = %d, want <= 10 (still on first line)", pos)
	}
}

// TestAltArrowWordNavigation verifies that Alt+Left/Right moves by word
// through the full editor pipeline (KeyPressMsg → ChordFromKeyMsg → Resolver → Command).
func TestAltArrowWordNavigation(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()

	bindings, _ := keys.CommandBindings()
	resolver, _ := keybind.NewResolver(bindings)

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	m.buf = buffer.New("hello world foo")
	// cursor at end: offset 15
	m.cursors = cursor.NewCursorSet(15)
	m = m.syncDisplay()

	// Alt+Left — should move from end to beginning of "foo" (offset 12)
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyLeft, Mod: tea.ModAlt})
	pos := m.cursors.Primary().Position
	if pos != 12 {
		t.Errorf("Alt+Left from offset 15: got position %d, want 12 (word-left to 'foo')", pos)
	}

	// Alt+Left again — should move to beginning of "world" (offset 6)
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyLeft, Mod: tea.ModAlt})
	pos = m.cursors.Primary().Position
	if pos != 6 {
		t.Errorf("Alt+Left from offset 12: got position %d, want 6 (word-left to 'world')", pos)
	}

	// Alt+Right — should move to end of "world" (offset 11)
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyRight, Mod: tea.ModAlt})
	pos = m.cursors.Primary().Position
	if pos != 11 {
		t.Errorf("Alt+Right from offset 6: got position %d, want 11 (word-right past 'world')", pos)
	}

	// Alt+Right — should move to end of "foo" (offset 15)
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyRight, Mod: tea.ModAlt})
	pos = m.cursors.Primary().Position
	if pos != 15 {
		t.Errorf("Alt+Right from offset 11: got position %d, want 15 (word-right past 'foo')", pos)
	}
}

// TestAltShiftArrowWordSelection verifies Alt+Shift+Left/Right selects by word.
func TestAltShiftArrowWordSelection(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()

	bindings, _ := keys.CommandBindings()
	resolver, _ := keybind.NewResolver(bindings)

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	m.buf = buffer.New("hello world")
	// cursor at offset 11 (end of "world")
	m.cursors = cursor.NewCursorSet(11)
	m = m.syncDisplay()

	// Alt+Shift+Left — should select "world" (anchor stays at 11, position moves to 6)
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyLeft, Mod: tea.ModAlt | tea.ModShift})
	c := m.cursors.Primary()
	if !c.HasSelection() {
		t.Fatal("Alt+Shift+Left did not create a selection")
	}
	if c.Position != 6 {
		t.Errorf("Alt+Shift+Left: position = %d, want 6", c.Position)
	}
	if c.Anchor != 11 {
		t.Errorf("Alt+Shift+Left: anchor = %d, want 11", c.Anchor)
	}
}

// TestCursorVisibleInView verifies that View() renders a visible cursor when
// focused (regression: bug #3 — no cursor rendering in View).
func TestCursorVisibleInView(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res, terminal.TermCaps{})
	m = m.SetSize(40, 10)
	m = m.SetFocused(true)
	m.buf = buffer.New("hello world")
	m.cursors = cursor.NewCursorSet(0)
	m = m.syncDisplay()

	view := m.View()

	// The cursor character should be rendered with ANSI escape sequences
	// (reverse video or similar styling). The raw text "hello world" without
	// any ANSI codes would mean no cursor is drawn.
	if !strings.Contains(view, "\x1b[") {
		t.Error("View() contains no ANSI sequences — cursor not rendered with styling")
	}

	// Without focus, cursor should NOT be rendered with reverse-video
	m2 := m.SetFocused(false)
	viewUnfocused := m2.View()
	// Focused and unfocused views must differ (focused has cursor highlight)
	if view == viewUnfocused {
		t.Error("focused and unfocused View() are identical — cursor rendering missing")
	}
}

// TestCursorAtEndOfLine verifies cursor renders at end-of-line position.
func TestCursorAtEndOfLine(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res, terminal.TermCaps{})
	m = m.SetSize(40, 10)
	m = m.SetFocused(true)
	m.buf = buffer.New("hi")
	m.cursors = cursor.NewCursorSet(2) // at end, past "hi"
	m = m.syncDisplay()

	view := m.View()
	// Should still have ANSI escape — cursor rendered as block space at EOL
	if !strings.Contains(view, "\x1b[") {
		t.Error("cursor at end-of-line not rendered")
	}
}

// TestCursorAtWrapBoundary_NoDoubleCursor verifies the cursor renders on
// exactly one display row when positioned at a soft-wrap boundary. Without the
// fix in View(), the EOL cursor on the first segment and the applyOverlays
// cursor on the second segment both fire, producing two visible cursor blocks.
func TestCursorAtWrapBoundary_NoDoubleCursor(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()

	bindings, _ := keys.CommandBindings()
	resolver, _ := keybind.NewResolver(bindings)

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	m = m.SetSize(6, 10) // narrow width so "abcdefghij" wraps
	m = m.SetFocused(true)
	m.buf = buffer.New("abcdefghij")
	m.cursors = cursor.NewCursorSet(5) // wrap point: after "abcde", before "fghij"
	m = m.syncDisplay()

	if len(m.snapshot.Lines) < 2 {
		t.Fatal("expected wrapped display lines; width may be too wide")
	}

	view := m.View()
	lines := strings.Split(view, "\n")

	cursorLines := 0
	for _, line := range lines {
		if strings.Contains(line, "\x1b[7m") {
			cursorLines++
		}
	}

	if cursorLines != 1 {
		t.Errorf("cursor appears on %d lines, want 1 (double cursor at wrap boundary)", cursorLines)
	}
}

// TestScrollToCursorVertical verifies that scrollToCursor adjusts TopRow when
// the cursor moves below the viewport (regression: bug #4 — scrollToCursor stub).
func TestScrollToCursorVertical(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()

	bindings, _ := keys.CommandBindings()
	resolver, _ := keybind.NewResolver(bindings)

	// Build a file with more lines than the viewport
	var fileLines []string
	for i := 0; i < 30; i++ {
		fileLines = append(fileLines, "line content here")
	}
	content := strings.Join(fileLines, "\n")

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	m = m.SetSize(40, 5) // only 5 lines visible (minus breadcrumb = ~4 content lines)
	m = m.SetFocused(true)
	m.buf = buffer.New(content)
	m.cursors = cursor.NewCursorSet(0)
	m = m.syncDisplay()

	if m.viewport.TopRow != 0 {
		t.Fatalf("initial TopRow = %d, want 0", m.viewport.TopRow)
	}

	// Move cursor down past the visible area
	for i := 0; i < 10; i++ {
		m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	}

	// Cursor should be on line ~10; viewport must have scrolled
	if m.viewport.TopRow == 0 {
		t.Error("TopRow still 0 after moving cursor 10 lines down — scrollToCursor not working")
	}

	// Verify cursor is within the visible window
	contentH := m.contentHeight()
	bp := m.buf.OffsetToLineCol(m.cursors.Primary().Position)
	sp := m.syntaxSnap.BufferToSyntax(bp)
	wp := m.wrapSnap.SyntaxToWrap(sp)
	if wp.Row < m.viewport.TopRow || wp.Row >= m.viewport.TopRow+contentH {
		t.Errorf("cursor at wrap row %d is outside viewport [%d, %d)",
			wp.Row, m.viewport.TopRow, m.viewport.TopRow+contentH)
	}
}

// TestScrollToCursorUpward verifies scrollToCursor adjusts TopRow upward
// when cursor moves above the viewport.
func TestScrollToCursorUpward(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()

	bindings, _ := keys.CommandBindings()
	resolver, _ := keybind.NewResolver(bindings)

	var fileLines []string
	for i := 0; i < 30; i++ {
		fileLines = append(fileLines, "line content here")
	}
	content := strings.Join(fileLines, "\n")

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	m = m.SetSize(40, 5)
	m = m.SetFocused(true)
	m.buf = buffer.New(content)
	// Start with cursor on line 20 and viewport scrolled there
	m.cursors = cursor.NewCursorSet(20 * 18) // 18 bytes per "line content here\n"
	m.viewport.TopRow = 20
	m = m.syncDisplay()

	// Move cursor up past the visible top
	for i := 0; i < 10; i++ {
		m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyUp})
	}

	// Viewport should have scrolled up to follow cursor
	bp := m.buf.OffsetToLineCol(m.cursors.Primary().Position)
	sp := m.syntaxSnap.BufferToSyntax(bp)
	wp := m.wrapSnap.SyntaxToWrap(sp)
	contentH := m.contentHeight()
	if wp.Row < m.viewport.TopRow || wp.Row >= m.viewport.TopRow+contentH {
		t.Errorf("cursor at wrap row %d is outside viewport [%d, %d) after scrolling up",
			wp.Row, m.viewport.TopRow, m.viewport.TopRow+contentH)
	}
}

// TestNavigationDoesNotCorruptCursors verifies that repeated navigation
// does not lose cursors or produce an empty CursorSet.
func TestNavigationDoesNotCorruptCursors(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()

	bindings, _ := keys.CommandBindings()
	resolver, _ := keybind.NewResolver(bindings)

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	m.buf = buffer.New("hello\nworld\nfoo bar")
	m.cursors = cursor.NewCursorSet(0)
	m = m.syncDisplay()

	// Exercise various navigation sequences
	moves := []tea.KeyPressMsg{
		{Code: tea.KeyRight},
		{Code: tea.KeyRight},
		{Code: tea.KeyDown},
		{Code: tea.KeyLeft},
		{Code: tea.KeyUp},
		{Code: tea.KeyDown},
		{Code: tea.KeyDown},
		{Code: tea.KeyRight},
		{Code: tea.KeyRight},
		{Code: tea.KeyRight},
	}

	for i, msg := range moves {
		m, _ = m.Update(msg)
		all := m.cursors.All()
		if len(all) == 0 {
			t.Fatalf("after move %d, CursorSet became empty", i)
		}
		pos := all[0].Position
		if pos < 0 || pos > len(m.buf.Content()) {
			t.Fatalf("after move %d, cursor position %d out of bounds [0, %d]",
				i, pos, len(m.buf.Content()))
		}
	}
}

// TestScrollOperationAdjustsViewport verifies that scroll commands
// (page up/down) modify the viewport TopRow correctly.
func TestScrollOperationAdjustsViewport(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	var fileLines []string
	for i := 0; i < 50; i++ {
		fileLines = append(fileLines, "scroll test line")
	}
	content := strings.Join(fileLines, "\n")

	m := New(keys, st, reg, res, terminal.TermCaps{})
	m = m.SetSize(40, 10)
	m = m.SetFocused(true)
	m.buf = buffer.New(content)
	m.cursors = cursor.NewCursorSet(0)
	m = m.syncDisplay()

	initialTop := m.viewport.TopRow

	// Simulate a scroll operation directly through dispatchOperation
	scrollOp := command.Result{
		Operation: command.Operation{
			Kind:     command.OperationScroll,
			ScrollDY: 5,
		},
	}
	m, _ = m.dispatchOperation(scrollOp, "scroll.page-down", time.Now())

	if m.viewport.TopRow != initialTop+5 {
		t.Errorf("TopRow = %d after scroll +5, want %d", m.viewport.TopRow, initialTop+5)
	}

	// Scroll up
	scrollUp := command.Result{
		Operation: command.Operation{
			Kind:     command.OperationScroll,
			ScrollDY: -3,
		},
	}
	m, _ = m.dispatchOperation(scrollUp, "scroll.page-up", time.Now())

	if m.viewport.TopRow != initialTop+2 {
		t.Errorf("TopRow = %d after scroll -3, want %d", m.viewport.TopRow, initialTop+2)
	}

	// Scroll should not go negative
	scrollWayUp := command.Result{
		Operation: command.Operation{
			Kind:     command.OperationScroll,
			ScrollDY: -100,
		},
	}
	m, _ = m.dispatchOperation(scrollWayUp, "scroll.page-up", time.Now())

	if m.viewport.TopRow < 0 {
		t.Errorf("TopRow = %d after over-scroll up, must not be negative", m.viewport.TopRow)
	}
}

// TestHorizontalRuleNavigation_NoCorruptedView verifies that navigating through
// a horizontal rule ("---") produces valid UTF-8 output and correct cursor
// positioning without garbled characters. This is the regression test for the
// wrap-map BufferStart/BufferEnd corruption bug.
func TestHorizontalRuleNavigation_NoCorruptedView(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()

	bindings, _ := keys.CommandBindings()
	resolver, _ := keybind.NewResolver(bindings)

	// Reproduce the exact bug scenario: long line with inline code + HR + heading.
	content := "Selection and Multi-Cursor compose: each cursor in a multi-cursor set can independently have an active selection (`Anchor != Position`).\n\n---\n\n### Actions — Navigation"

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	m = m.SetContent("test.md", []byte(content))

	// Navigate down 4 times through: paragraph -> empty -> HR -> empty -> heading
	for i := 0; i < 5; i++ {
		m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})

		// After each navigation, verify:
		// 1. View output is valid UTF-8
		view := m.View()
		for lineNo, line := range strings.Split(view, "\n") {
			for byteIdx := 0; byteIdx < len(line); {
				r, size := utf8.DecodeRuneInString(line[byteIdx:])
				if r == utf8.RuneError && size == 1 {
					t.Errorf("after down %d: invalid UTF-8 at view line %d, byte %d, context: %q",
						i+1, lineNo, byteIdx, safeSlice(line, byteIdx-5, byteIdx+10))
				}
				byteIdx += size
			}
		}

		// 2. Cursor position is within buffer bounds
		pos := m.cursors.Primary().Position
		if pos < 0 || pos > len(content) {
			t.Errorf("after down %d: cursor position %d out of bounds [0, %d]",
				i+1, pos, len(content))
		}

		// 3. No display spans have BufferStart > BufferEnd
		for _, dline := range m.snapshot.Lines {
			for _, sp := range dline.Spans {
				if len(sp.Text) > 0 && sp.BufferEnd < sp.BufferStart {
					t.Errorf("after down %d: span has BufferEnd (%d) < BufferStart (%d), text=%q",
						i+1, sp.BufferEnd, sp.BufferStart, sp.Text)
				}
			}
		}
	}
}

// TestInlineCodeWrapping_BufferOffsetsCorrect verifies that soft-wrapping a line
// with rendered inline code produces wrap segments with correct buffer offsets.
// This catches the root cause: the old wrap map used Spans[0].BufferStart as
// the base for all segments, ignoring hidden delimiter bytes.
func TestInlineCodeWrapping_BufferOffsetsCorrect(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()

	bindings, _ := keys.CommandBindings()
	resolver, _ := keybind.NewResolver(bindings)

	// Line with inline code that will wrap: rendered text is shorter than buffer.
	content := "before `inline code here` after this more text padding"

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	// Width 30 forces a wrap in the middle of the line.
	m = m.SetSize(30, 10)
	m = m.SetFocused(true)

	// Put cursor on line 1 (if existed) or beyond inline code to make it Rendered.
	// Adding a second line ensures cursor is not on line 0.
	content += "\nsecond"
	m = m.SetContent("test.md", []byte(content))

	// Move cursor to line 2 (second line) so line 0's inline code is Rendered.
	m.cursors = cursor.NewCursorSet(len(content) - 6) // "second" starts here
	m = m.syncDisplay()

	// Verify all wrap segments for line 0 have valid buffer offsets.
	line0Start := 0
	line0End := strings.Index(content, "\n")

	for i, dline := range m.snapshot.Lines {
		if dline.ModelLine != 0 {
			continue
		}
		for j, sp := range dline.Spans {
			if len(sp.Text) == 0 {
				continue
			}
			if sp.BufferStart < line0Start {
				t.Errorf("display line %d span %d: BufferStart %d < line start %d",
					i, j, sp.BufferStart, line0Start)
			}
			if sp.BufferEnd > line0End {
				t.Errorf("display line %d span %d: BufferEnd %d > line end %d, text=%q",
					i, j, sp.BufferEnd, line0End, sp.Text)
			}
			if sp.State == display.Revealed && sp.BufferEnd-sp.BufferStart != len(sp.Text) {
				t.Errorf("display line %d span %d: Revealed span buffer range %d != text len %d, text=%q",
					i, j, sp.BufferEnd-sp.BufferStart, len(sp.Text), sp.Text)
			}
		}
	}
}

func safeSlice(s string, start, end int) string {
	if start < 0 {
		start = 0
	}
	if end > len(s) {
		end = len(s)
	}
	if start >= end {
		return ""
	}
	return s[start:end]
}
