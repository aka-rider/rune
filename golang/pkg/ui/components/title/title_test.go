package title

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/internal/editortest"
	"rune/pkg/command"
	"rune/pkg/editor/keybind"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func testOpts() []textedit.Option {
	keys := keymap.Default()
	builder := command.NewBuilder()
	builder, _ = textedit.RegisterCommands(builder)
	reg := builder.Build()
	bindings, _ := keys.CommandBindings()
	resolver, _ := keybind.NewResolver(bindings)
	return []textedit.Option{
		textedit.WithRegistry(reg),
		textedit.WithResolver(resolver),
	}
}

func newTestTitle() Model {
	return New("Untitled 1", keymap.Default(), styles.Default(), testOpts()...)
}

func typeText(m Model, s string) Model {
	return editortest.TypeText(m, Model.Update, s)
}

// retype focuses the title with the whole current name selected (the real
// ^n/rename entry point) and types s over it — the behavioral replacement
// for the old direct m.field.SetContent pokes.
func retype(m Model, s string) Model {
	m = m.FocusAndSelectAll()
	return typeText(m, s)
}

// --- Basic text / state ---

func TestTitle_DefaultText(t *testing.T) {
	m := newTestTitle()
	if m.Text() != "Untitled 1" {
		t.Errorf("expected 'Untitled 1', got %q", m.Text())
	}
	// "Untitled 1" is the constructor's placeholder argument — the default
	// text must be exactly it.
}

func TestTitle_SetText(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("my-note")
	if m.Text() != "my-note" {
		t.Errorf("expected 'my-note', got %q", m.Text())
	}
	if m.Text() == "Untitled 1" {
		t.Error("expected the text to no longer equal the constructor placeholder after SetText")
	}
}

func TestTitle_TypeChar(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m = m.SetText("")
	m = typeText(m, "hi")
	if m.Text() != "hi" {
		t.Errorf("expected 'hi', got %q", m.Text())
	}
}

// TestTitle_SetFocusedIsIdempotent verifies SetFocused(true) has no cursor side
// effect, so the workspace projecting focus on every frame cannot disrupt mid-title
// editing (regression: focus desync fix relies on this idempotence).
func TestTitle_SetFocusedIsIdempotent(t *testing.T) {
	m := newTestTitle()
	m = m.FocusAtEnd()
	m = m.SetText("ab") // focused → cursor at end (after 'b')
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyLeft})
	m = m.SetFocused(true) // simulates per-frame projection; must NOT jump cursor to end
	m = typeText(m, "X")
	if got := m.Text(); got != "aXb" {
		t.Fatalf("SetFocused moved the cursor: got %q, want %q", got, "aXb")
	}
}

// TestTitle_SelectAllSurvivesProjection verifies the ^n select-all is not cleared by
// the per-frame SetFocused projection, so typing replaces the whole name.
func TestTitle_SelectAllSurvivesProjection(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("hello")
	m = m.FocusAndSelectAll()
	m = m.SetFocused(true) // per-frame projection — selection must survive
	m = typeText(m, "X")
	if got := m.Text(); got != "X" {
		t.Fatalf("select-all lost after projection: typing gave %q, want %q", got, "X")
	}
}

func TestTitle_Backspace(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m = m.SetText("abc")
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyBackspace})
	if m.Text() != "ab" {
		t.Errorf("expected 'ab', got %q", m.Text())
	}
}

// --- Commit ---

func TestTitle_CommitEmitsRename(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("original")
	m = retype(m, "new-name") // real select-all + typing, not a field poke

	m, cmd := m.Commit()
	if cmd == nil {
		t.Fatal("expected a Cmd from Commit when text changed")
	}
	result := cmd()
	rename, ok := result.(RenameRequestMsg)
	if !ok {
		t.Fatalf("expected RenameRequestMsg, got %T", result)
	}
	if rename.Name != "new-name" {
		t.Errorf("expected Name='new-name', got %q", rename.Name)
	}
	// Behavior of a landed commit: the new name is now the baseline, so an
	// immediate second Commit has nothing to do.
	if _, again := m.Commit(); again != nil {
		t.Error("expected nil Cmd from a second Commit — the first must have updated the committed baseline")
	}
}

func TestTitle_CommitNoOp(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("same")
	_, cmd := m.Commit()
	if cmd != nil {
		t.Error("expected nil Cmd when text unchanged")
	}
}

// --- Down / Enter ---

func TestTitle_DownReturnsFocus(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("original")
	m = retype(m, "changed")

	m, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if m.Focused() {
		t.Error("expected Focused()=false after Down")
	}
	if cmd == nil {
		t.Fatal("expected a non-nil Cmd from Down")
	}
	// Down commits: the emitted messages must include the rename.
	var rename *RenameRequestMsg
	for _, msg := range editortest.ExecCmds(cmd) {
		if r, ok := msg.(RenameRequestMsg); ok {
			rename = &r
		}
	}
	if rename == nil || rename.Name != "changed" {
		t.Fatalf("expected Down to emit RenameRequestMsg{Name:\"changed\"}, got %+v", rename)
	}
}

func TestTitle_DownNoRenameWhenUnchanged(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("same")
	m = m.SetFocused(true)

	m, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if m.Focused() {
		t.Error("expected Focused()=false after Down")
	}
	for _, msg := range editortest.ExecCmds(cmd) {
		if _, ok := msg.(RenameRequestMsg); ok {
			t.Fatal("Down with unchanged text must not emit RenameRequestMsg")
		}
	}
	if m.Text() != "same" {
		t.Errorf("expected Text()='same', got %q", m.Text())
	}
}

// --- Escape ---

func TestTitle_EscapeReverts(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("committed-name")
	m = retype(m, "changed")

	m, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	if m.Text() != "committed-name" {
		t.Errorf("expected revert to 'committed-name', got %q", m.Text())
	}
	if m.Focused() {
		t.Error("expected Focused()=false after Escape")
	}
	if cmd == nil {
		t.Fatal("expected FocusReturnMsg cmd from Escape")
	}
	result := cmd()
	if _, ok := result.(FocusReturnMsg); !ok {
		t.Fatalf("expected FocusReturnMsg, got %T", result)
	}
}

func TestTitle_EscapeNoRename(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("committed-name")
	m = retype(m, "changed")

	m, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	// Escape reverts — no rename may be emitted, and the abandoned text
	// must not have become the committed baseline (a later Commit at the
	// reverted text has nothing to do).
	for _, msg := range editortest.ExecCmds(cmd) {
		if _, ok := msg.(RenameRequestMsg); ok {
			t.Fatal("Escape must not emit RenameRequestMsg")
		}
	}
	if _, again := m.Commit(); again != nil {
		t.Error("expected nil Cmd from Commit after Escape — the revert must have restored the committed baseline")
	}
}

// --- Filename filtering ---

func TestTitle_InputFiltersInvalidChars(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m = m.SetText("")

	// '/' is invalid — should be dropped silently.
	m, _ = m.Update(tea.KeyPressMsg{Text: "/"})
	if m.Text() != "" {
		t.Errorf("expected '/' to be filtered, got %q", m.Text())
	}

	m, _ = m.Update(tea.KeyPressMsg{Text: "a"})
	if m.Text() != "a" {
		t.Errorf("expected 'a', got %q", m.Text())
	}
}

func TestTitle_PasteReplacesInvalidChars(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m = m.SetText("")

	m, _ = m.Update(tea.ClipboardMsg{Content: "my/note:file"})
	if m.Text() != "my_note_file" {
		t.Errorf("expected 'my_note_file', got %q", m.Text())
	}
}

func TestTitle_PasteIgnoredWhenUnfocused(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("original")

	m, _ = m.Update(tea.ClipboardMsg{Content: "intruder"})
	if m.Text() != "original" {
		t.Errorf("expected text unchanged, got %q", m.Text())
	}
}

func TestTitle_BracketedPasteInsertsText(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m = m.SetText("")

	m, _ = m.Update(tea.PasteMsg{Content: "my-note"})
	if m.Text() != "my-note" {
		t.Errorf("expected 'my-note' after bracketed paste, got %q", m.Text())
	}
}

func TestTitle_BracketedPasteReplacesInvalidChars(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m = m.SetText("")

	m, _ = m.Update(tea.PasteMsg{Content: "my/note:file"})
	if m.Text() != "my_note_file" {
		t.Errorf("expected 'my_note_file', got %q", m.Text())
	}
}

func TestTitle_BracketedPasteIgnoredWhenUnfocused(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("original")

	m, _ = m.Update(tea.PasteMsg{Content: "intruder"})
	if m.Text() != "original" {
		t.Errorf("expected text unchanged when unfocused, got %q", m.Text())
	}
}

// --- Modifier guard ---

func TestTitle_CmdCDoesNotPrintC(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m = m.SetText("hello")

	// Simulate Cmd+C: ModSuper + 'c' — must not insert 'c'.
	m, _ = m.Update(tea.KeyPressMsg{Code: 'c', Mod: tea.ModSuper, Text: "c"})
	if m.Text() != "hello" {
		t.Errorf("Cmd+C must not insert 'c', got %q", m.Text())
	}
}

func TestTitle_CmdCReturnsCopyCmd(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m = m.SetText("hello")

	_, cmd := m.Update(tea.KeyPressMsg{Code: 'c', Mod: tea.ModSuper, Text: "c"})
	if cmd == nil {
		t.Fatal("Cmd+C must return a non-nil Cmd (clipboard write)")
	}
}

func TestTitle_CmdVReturnsPasteCmd(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m = m.SetText("hello")

	_, cmd := m.Update(tea.KeyPressMsg{Code: 'v', Mod: tea.ModSuper, Text: "v"})
	if cmd == nil {
		t.Fatal("Cmd+V must return a non-nil Cmd (clipboard read request)")
	}
}

func TestTitle_CmdZDoesNotPrintZ(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m = m.SetText("hello")

	// Simulate Cmd+Z (undo): ModSuper + 'z' — must not insert 'z'.
	m, _ = m.Update(tea.KeyPressMsg{Code: 'z', Mod: tea.ModSuper})
	if m.Text() != "hello" {
		t.Errorf("Cmd+Z must not insert 'z', got %q", m.Text())
	}
}

func TestTitle_CtrlZDoesNotPrintZ(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m = m.SetText("hello")

	m, _ = m.Update(tea.KeyPressMsg{Code: 'z', Mod: tea.ModCtrl})
	if m.Text() != "hello" {
		t.Errorf("Ctrl+Z must not insert 'z', got %q", m.Text())
	}
}

// --- Keys ignored when unfocused ---

func TestTitle_KeyIgnoredWhenUnfocused(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("hello")

	m, _ = m.Update(tea.KeyPressMsg{Text: "x"})
	if m.Text() != "hello" {
		t.Errorf("expected text unchanged when unfocused, got %q", m.Text())
	}
}

// --- View ---

func TestTitle_View(t *testing.T) {
	m := newTestTitle()
	m = m.SetSize(80, 1)
	view := m.View()
	if view == "" {
		t.Error("expected non-empty view")
	}
}
