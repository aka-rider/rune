// Package search provides the in-file search bar component. It owns the query
// input field, match-count status display, and ↑/↓ history navigation.
// The workspace owns the match list and database I/O; this component only
// emits SubmitMsg and CloseMsg for the workspace to act on.
package search

import (
	"fmt"
	"unicode/utf8"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	edSearch "rune/pkg/editor/search"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// SubmitMsg is emitted when the user presses Enter (or Shift+Enter) to
// navigate to a match. Backward is true for Shift+Enter (find previous).
type SubmitMsg struct {
	Query    string
	Backward bool
}

// CloseMsg is emitted when the user presses Escape to close the search bar.
type CloseMsg struct{}

const promptGlyph = "/ " // leading prompt rendered before the input field

// Model is the search bar component.
type Model struct {
	field   textedit.Model
	visible bool
	width   int
	status  string // e.g. "3/12" or "no matches"
	styles  styles.Styles
	keys    keymap.Bindings

	// History navigation (in-memory; workspace supplies entries via SetHistory).
	history    []string // recent-first
	workingSet []string // filtered by fuzzy match on draft; set when entering history
	histIdx    int      // -1 = editing live draft; 0..n-1 = into workingSet
	draft      string   // user's typed text preserved across history navigation
}

// New creates a search bar in the closed state.
func New(keys keymap.Bindings, st styles.Styles) Model {
	field := textedit.New(keys, st, textedit.WithSingleLine())
	field = field.SetRect(textedit.Rect{W: 80, H: 1})
	return Model{
		field:   field,
		visible: false,
		histIdx: -1,
		styles:  st,
		keys:    keys,
	}
}

func (m Model) Init() tea.Cmd { return nil }

// Height returns 1 when visible, 0 when hidden.
func (m Model) Height() int {
	if m.visible {
		return 1
	}
	return 0
}

// Visible reports whether the bar is currently shown.
func (m Model) Visible() bool { return m.visible }

// Query returns the current input text.
func (m Model) Query() string {
	c := m.field.Content()
	// Trim any trailing newline that textedit may add.
	for len(c) > 0 && (c[len(c)-1] == '\n' || c[len(c)-1] == '\r') {
		c = c[:len(c)-1]
	}
	return c
}

// Open shows the bar and resets state for a fresh search.
func (m Model) Open() Model {
	m.visible = true
	m.histIdx = -1
	m.draft = ""
	m.status = ""
	m.workingSet = nil
	m.field = m.field.SetContent("")
	m.field = m.field.SetFocused(true)
	return m
}

// Close hides the bar.
func (m Model) Close() Model {
	m.visible = false
	m.field = m.field.SetFocused(false)
	return m
}

// SetFocused is the idempotent focus-state setter called by workspace.applyFocus
// on every Update pass.
func (m Model) SetFocused(v bool) Model {
	m.field = m.field.SetFocused(v)
	return m
}

// SetSize allocates width and recomputes the field width accordingly.
func (m Model) SetSize(w, _ int) Model {
	m.width = w
	return m.syncFieldWidth()
}

// SetHistory replaces the history list (workspace calls this after a DB load).
func (m Model) SetHistory(h []string) Model {
	m.history = h
	return m
}

// SetStatus replaces the right-aligned status text ("3/12", "no matches", "").
func (m Model) SetStatus(s string) Model {
	m.status = s
	return m.syncFieldWidth()
}

// syncFieldWidth recomputes and applies the field width based on current
// width, prompt, and status widths. Must not be called from View.
func (m Model) syncFieldWidth() Model {
	promptW := utf8.RuneCountInString(promptGlyph)
	statusW := 0
	if m.status != "" {
		// " 3/12" — one space prefix
		statusW = 1 + utf8.RuneCountInString(m.status)
	}
	fieldW := m.width - promptW - statusW
	if fieldW < 1 {
		fieldW = 1
	}
	m.field = m.field.SetRect(textedit.Rect{W: fieldW, H: 1})
	return m
}

// Update handles messages. Key input is only processed when the bar is focused.
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	if !m.visible {
		return m, nil
	}

	switch msg := msg.(type) {
	case tea.ClipboardMsg:
		if !m.field.Focused() {
			return m, nil
		}
		var cmd tea.Cmd
		m.field, cmd = m.field.Update(msg)
		m.histIdx = -1
		m.draft = m.Query()
		return m, cmd

	case tea.PasteMsg:
		if !m.field.Focused() {
			return m, nil
		}
		var cmd tea.Cmd
		m.field, cmd = m.field.Update(msg)
		m.histIdx = -1
		m.draft = m.Query()
		return m, cmd

	case tea.KeyPressMsg:
		if !m.field.Focused() {
			return m, nil
		}
		return m.handleKey(msg)
	}

	return m, nil
}

func (m Model) handleKey(msg tea.KeyPressMsg) (Model, tea.Cmd) {
	// Escape → close
	if msg.Code == tea.KeyEscape && msg.Mod == 0 {
		return m, func() tea.Msg { return CloseMsg{} }
	}

	// Shift+Enter → submit backward
	if msg.Code == tea.KeyEnter && msg.Mod == tea.ModShift {
		q := m.Query()
		m.histIdx = -1
		m.draft = q
		return m, func() tea.Msg { return SubmitMsg{Query: q, Backward: true} }
	}

	// Enter → submit forward
	if msg.Code == tea.KeyEnter && msg.Mod == 0 {
		q := m.Query()
		m.histIdx = -1
		m.draft = q
		return m, func() tea.Msg { return SubmitMsg{Query: q, Backward: false} }
	}

	// Up → history: walk toward most-recent (then toward oldest)
	if msg.Code == tea.KeyUp && msg.Mod == 0 {
		return m.historyUp(), nil
	}

	// Down → history: walk toward oldest (then back toward draft)
	if msg.Code == tea.KeyDown && msg.Mod == 0 {
		return m.historyDown(), nil
	}

	// Any other key → forward to field and exit history navigation
	prevQuery := m.Query()
	var cmd tea.Cmd
	m.field, cmd = m.field.Update(msg)
	if m.Query() != prevQuery {
		m.histIdx = -1
		m.draft = m.Query()
		m.workingSet = nil
	}
	return m, cmd
}

// ---- History navigation state machine (§7) ----

// historyUp moves toward the most-recent end on the first press, then toward
// the least-recent end on subsequent presses.
//
//	histIdx == -1 : compute workingSet; if empty → no-op; else histIdx=0
//	else          : histIdx = min(histIdx+1, len-1)
func (m Model) historyUp() Model {
	if m.histIdx == -1 {
		m.workingSet = m.computeWorkingSet()
		if len(m.workingSet) == 0 {
			return m
		}
		m.histIdx = 0
	} else {
		if m.histIdx < len(m.workingSet)-1 {
			m.histIdx++
		}
	}
	m.field = m.field.SetContent(m.workingSet[m.histIdx])
	return m
}

// historyDown enters at the least-recent end on the first press, then walks
// toward the most-recent end; past the most-recent end returns to the draft.
//
//	histIdx == -1 : compute workingSet; if empty → no-op; else histIdx=len-1
//	else          : histIdx--; if < 0 → exit to draft (histIdx=-1, input=draft)
func (m Model) historyDown() Model {
	if m.histIdx == -1 {
		m.workingSet = m.computeWorkingSet()
		if len(m.workingSet) == 0 {
			return m
		}
		m.histIdx = len(m.workingSet) - 1
	} else {
		m.histIdx--
		if m.histIdx < 0 {
			m.histIdx = -1
			m.field = m.field.SetContent(m.draft)
			return m
		}
	}
	m.field = m.field.SetContent(m.workingSet[m.histIdx])
	return m
}

// computeWorkingSet returns the full history when draft is empty; otherwise the
// subsequence-filtered subset (recent-first order preserved).
func (m Model) computeWorkingSet() []string {
	if m.draft == "" {
		return m.history
	}
	var ws []string
	for _, h := range m.history {
		if edSearch.FuzzyMatch(m.draft, h) {
			ws = append(ws, h)
		}
	}
	return ws
}

// View renders the bar: prompt + field + right-aligned status.
func (m Model) View() string {
	if !m.visible {
		return ""
	}

	prompt := lipgloss.NewStyle().Render(promptGlyph)
	fieldView := m.field.View()

	var statusPart string
	if m.status != "" {
		statusPart = " " + m.status
	}

	content := prompt + fieldView + statusPart
	return lipgloss.NewStyle().MaxWidth(m.width).Render(content)
}

// statusFor builds a status string from match count values.
// Returns empty string when there are no matches.
func StatusFor(idx, total int) string {
	if total == 0 {
		return "no matches"
	}
	if idx == 0 {
		return fmt.Sprintf("%d matches", total)
	}
	return fmt.Sprintf("%d/%d", idx, total)
}
