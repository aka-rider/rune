package title

import (
	"testing"

	tea "charm.land/bubbletea/v2"

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
	for _, r := range s {
		m, _ = m.Update(tea.KeyPressMsg{Text: string(r)})
	}
	return m
}

// --- Basic text / state ---

func TestTitle_DefaultText(t *testing.T) {
	m := newTestTitle()
	if m.Text() != "Untitled 1" {
		t.Errorf("expected 'Untitled 1', got %q", m.Text())
	}
	if !m.IsPlaceholder() {
		t.Error("expected IsPlaceholder to be true")
	}
}

func TestTitle_SetText(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("my-note")
	if m.Text() != "my-note" {
		t.Errorf("expected 'my-note', got %q", m.Text())
	}
	if m.IsPlaceholder() {
		t.Error("expected IsPlaceholder to be false after SetText")
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
	m = m.SetFocused(true)
	m = typeText(m, "x") // changes text to "originalx" (cursor at end after SetText)
	m.field = m.field.SetContent("new-name")

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
	if m.committed != "new-name" {
		t.Errorf("expected committed='new-name', got %q", m.committed)
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
	m = m.SetFocused(true)
	m.field = m.field.SetContent("changed")

	m, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if m.focused {
		t.Error("expected focused=false after Down")
	}
	if cmd == nil {
		t.Fatal("expected a non-nil Cmd from Down")
	}
	// Down commits: m.committed must reflect the new text.
	if m.committed != "changed" {
		t.Errorf("expected committed='changed' after Down, got %q", m.committed)
	}
}

func TestTitle_DownNoRenameWhenUnchanged(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("same")
	m = m.SetFocused(true)

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyDown})
	if m.focused {
		t.Error("expected focused=false after Down")
	}
	if m.committed != "same" {
		t.Errorf("expected committed='same', got %q", m.committed)
	}
}

// --- Escape ---

func TestTitle_EscapeReverts(t *testing.T) {
	m := newTestTitle()
	m = m.SetText("committed-name")
	m = m.SetFocused(true)
	m.field = m.field.SetContent("changed")

	m, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	if m.Text() != "committed-name" {
		t.Errorf("expected revert to 'committed-name', got %q", m.Text())
	}
	if m.focused {
		t.Error("expected focused=false after Escape")
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
	m = m.SetFocused(true)
	m.field = m.field.SetContent("changed")

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	// Escape reverts: committed unchanged.
	if m.committed != "committed-name" {
		t.Errorf("committed changed on Escape — expected 'committed-name', got %q", m.committed)
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
	m, _ = m.Update(tea.KeyPressMsg{Code: 'z', Mod: tea.ModSuper, Text: "z"})
	if m.Text() != "hello" {
		t.Errorf("Cmd+Z must not insert 'z', got %q", m.Text())
	}
}

func TestTitle_CtrlZDoesNotPrintZ(t *testing.T) {
	m := newTestTitle()
	m = m.SetFocused(true)
	m = m.SetText("hello")

	m, _ = m.Update(tea.KeyPressMsg{Code: 'z', Mod: tea.ModCtrl, Text: "z"})
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
