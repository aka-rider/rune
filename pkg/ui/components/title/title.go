package title

import (
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/bubbles/v2/key"
	"charm.land/lipgloss/v2"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// RenameRequestMsg is emitted when a committed title edit should trigger a
// file rename. Produced by Commit() — on Down/Enter or explicit blur.
type RenameRequestMsg struct{ Name string }

// FocusReturnMsg is emitted when the user navigates away from the title
// (Down arrow, Enter, or Escape), returning focus to the editor content.
type FocusReturnMsg struct{}

// invalidFileNameChars is the set of characters not allowed in filenames.
const invalidFileNameChars = "/\\:*?\"<>|\x00"

type Model struct {
	field       textedit.Model
	committed   string // last value sent to disk; revert target on Escape
	placeholder string
	focused     bool
	width       int
	styles      styles.Styles
	keys        keymap.Bindings
}

func New(placeholder string, keys keymap.Bindings, st styles.Styles, opts ...textedit.Option) Model {
	allOpts := append([]textedit.Option{textedit.WithSingleLine()}, opts...)
	field := textedit.New(keys, st, allOpts...)
	field = field.SetContent(placeholder)
	field = field.SetRect(textedit.Rect{W: 80, H: 1})
	return Model{
		field:       field,
		committed:   placeholder,
		placeholder: placeholder,
		styles:      st,
		keys:        keys,
	}
}

func (m Model) Init() tea.Cmd { return nil }
func (m Model) Height() int   { return 1 }

func (m Model) Text() string {
	return strings.TrimRight(m.field.Content(), "\n")
}

func (m Model) Focused() bool { return m.focused }

// IsPlaceholder reports whether the title is still the original placeholder.
func (m Model) IsPlaceholder() bool { return m.Text() == m.placeholder }

func (m Model) SetSize(w, _ int) Model {
	m.width = w
	m.field = m.field.SetRect(textedit.Rect{W: w, H: 1})
	return m
}

func (m Model) SetText(name string) Model {
	m.field = m.field.SetContent(name)
	m.committed = name
	// SetContent resets the cursor to position 0. If the title is focused,
	// move it to the end so edits feel natural (matches old implementation's
	// cursor = len(text) on SetFocused).
	if m.focused {
		m.field, _ = m.field.Update(tea.KeyPressMsg{Code: tea.KeyEnd})
	}
	return m
}

// SetFocused is the idempotent focus-state setter. It is projected from the
// workspace's focus authority on every Update pass, so it MUST have no cursor
// side effects — focus-gain gestures live in FocusAtEnd / FocusAndSelectAll.
func (m Model) SetFocused(v bool) Model {
	m.focused = v
	m.field = m.field.SetFocused(v)
	return m
}

// FocusAtEnd focuses the title and lands the cursor at the end of the text — the
// natural entry point when arriving from the editor (D11) or a click. This is a
// focus-gain gesture invoked once at the transition, distinct from the idempotent
// SetFocused projected each frame.
func (m Model) FocusAtEnd() Model {
	m = m.SetFocused(true)
	m.field, _ = m.field.Update(tea.KeyPressMsg{Code: tea.KeyEnd})
	return m
}

// FocusAndSelectAll focuses the title and pre-selects all text so the user
// can type a replacement name without a separate select-all step.
func (m Model) FocusAndSelectAll() Model {
	m.focused = true
	m.field = m.field.SetFocused(true)
	m.field = m.field.MouseSelectLine(0)
	return m
}

// DrainEdits forwards to the underlying field, returning title.Model.
func (m Model) DrainEdits() (Model, []buffer.AppliedEdit) {
	var edits []buffer.AppliedEdit
	m.field, edits = m.field.DrainEdits()
	return m, edits
}

// Cursors returns the cursor state of the underlying field.
func (m Model) Cursors() []cursor.Cursor {
	return m.field.Cursors()
}

// ApplyInverse applies inverse edits to the underlying field (workspace-driven undo).
func (m Model) ApplyInverse(edits []buffer.AppliedEdit) Model {
	m.field = m.field.ApplyInverse(edits)
	return m
}

// Reapply applies edits forward to the underlying field (workspace-driven redo).
func (m Model) Reapply(edits []buffer.AppliedEdit) Model {
	m.field = m.field.Reapply(edits)
	return m
}

// SetCursors restores cursor state on the underlying field.
func (m Model) SetCursors(cs []cursor.Cursor) Model {
	m.field = m.field.SetCursors(cs)
	return m
}

// Commit emits RenameRequestMsg if the text has changed since last commit.
func (m Model) Commit() (Model, tea.Cmd) {
	text := m.Text()
	if text == m.committed {
		return m, nil
	}
	m.committed = text
	return m, func() tea.Msg { return RenameRequestMsg{Name: text} }
}

// --- Filename sanitisation ---

func isInvalidFileNameChar(r rune) bool {
	for _, c := range invalidFileNameChars {
		if r == c {
			return true
		}
	}
	return r < 32
}

// filterFileName returns s with each invalid filename character replaced by
// replacement. Pass "" to drop invalid chars silently.
func filterFileName(s, replacement string) string {
	if s == "" {
		return s
	}
	var out []rune
	rep := []rune(replacement)
	for _, r := range s {
		if isInvalidFileNameChar(r) {
			out = append(out, rep...)
		} else {
			out = append(out, r)
		}
	}
	return string(out)
}

// --- Update ---

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.ClipboardMsg:
		// OSC 52 clipboard read response — arrives after clipboard.paste Cmd fires.
		if !m.focused {
			return m, nil
		}
		filtered := tea.ClipboardMsg{Content: filterFileName(msg.Content, "_")}
		var cmd tea.Cmd
		m.field, cmd = m.field.Update(filtered)
		return m, cmd

	case tea.PasteMsg:
		// Bracketed paste — macOS terminals send this on Cmd+V directly instead
		// of forwarding the key and waiting for OSC 52.
		if !m.focused {
			return m, nil
		}
		filtered := tea.PasteMsg{Content: filterFileName(msg.Content, "_")}
		var cmd tea.Cmd
		m.field, cmd = m.field.Update(filtered)
		return m, cmd

	case tea.KeyPressMsg:
		if !m.focused {
			return m, nil
		}
		return m.handleKey(msg)
	}
	return m, nil
}

func (m Model) handleKey(msg tea.KeyPressMsg) (Model, tea.Cmd) {
	// Down / Enter — commit and return focus to editor.
	if msg.Code == tea.KeyDown && msg.Mod == 0 ||
		msg.Code == tea.KeyEnter && msg.Mod == 0 {
		m.focused = false
		m.field = m.field.SetFocused(false)
		var renameCmd tea.Cmd
		m, renameCmd = m.Commit()
		return m, tea.Batch(renameCmd, func() tea.Msg { return FocusReturnMsg{} })
	}

	// Escape — revert to committed value and return focus.
	if msg.Code == tea.KeyEscape && msg.Mod == 0 {
		m.field = m.field.SetContent(m.committed)
		m.field = m.field.SetFocused(false)
		m.focused = false
		return m, func() tea.Msg { return FocusReturnMsg{} }
	}

	// Undo/Redo are handled at the workspace level. Consume them here so
	// they never reach the textedit field (which would insert 'z' or 'y').
	if key.Matches(msg, m.keys.Undo) || key.Matches(msg, m.keys.Redo) {
		return m, nil
	}

	// Filter invalid filename chars from text input.
	// Only applies when no command modifier (Ctrl, Alt, Super) is held —
	// those are shortcuts (undo, copy, etc.) and must reach textedit unmodified.
	if msg.Text != "" && msg.Mod&^tea.ModShift == 0 {
		filtered := filterFileName(msg.Text, "")
		if filtered == "" {
			return m, nil // blocked
		}
		msg.Text = filtered
	}

	var cmd tea.Cmd
	m.field, cmd = m.field.Update(msg)
	return m, cmd
}

// --- View ---

func (m Model) View() string {
	titleStyle := lipgloss.NewStyle().
		Foreground(lipgloss.Color("216")).
		Bold(true).
		Padding(0, 1)

	if !m.focused {
		content := titleStyle.MaxWidth(m.width).Render(m.Text())
		return content
	}

	// Focused: textedit renders cursor and selection natively.
	return lipgloss.NewStyle().
		MaxWidth(m.width).
		Padding(0, 1).
		Render(m.field.View())
}
