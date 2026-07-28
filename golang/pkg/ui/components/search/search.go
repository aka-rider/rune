// Package search provides the in-file search bar component. It owns the query
// input field, match-count status display, and ↑/↓ history navigation.
// The workspace owns the match list and database I/O; this component only
// emits SubmitMsg and CloseMsg for the workspace to act on.
package search

import (
	"fmt"
	"unicode/utf8"

	"charm.land/bubbles/v2/key"
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

// historyReadyMsg is the internal async response from a DB history load.
// dir: +1 = Up (enter at most-recent); -1 = Down (enter at least-recent).
type historyReadyMsg struct {
	entries []string
	draft   string
	dir     int
}

// historyNav tracks search-history browsing state (ephemeral; reset on
// Open/Close). §1.7: replaces the old entries==nil ("not yet loaded") /
// idx==-1 ("editing the live draft") sentinels with explicit fields, mirroring
// the blessed textedit.ActiveMatch{Index, Valid} shape — one value, one
// meaning, so a missed nil/-1 check can't silently misread the state.
type historyNav struct {
	entries  []string // history entries filtered by fuzzy match on draft
	loaded   bool     // true once a DB load's result has been applied via applyHistoryReady
	browsing bool     // true while navigating history; false = editing the live draft
	idx      int      // index into entries; meaningful only while browsing
}

// Model is the search bar component.
type Model struct {
	field   textedit.Model
	visible bool
	width   int
	status  string // e.g. "3/12" or "no matches"
	styles  styles.Styles
	keys    keymap.Bindings

	// historyLoader is injected by the workspace after the store is ready.
	// Nil until wired; navigation is a no-op while nil.
	historyLoader func() ([]string, error)

	hist  historyNav // history navigation state (ephemeral; reset on Open/Close)
	draft string     // user's typed text preserved across history navigation

	// undoStack stores previous query snapshots for Cmd+Z within the search field.
	undoStack []string
}

// New creates a search bar in the closed state. Pass textedit.WithRegistry and
// textedit.WithResolver so the field can process character input and backspace.
func New(keys keymap.Bindings, st styles.Styles, opts ...textedit.Option) Model {
	allOpts := append([]textedit.Option{textedit.WithSingleLine()}, opts...)
	field := textedit.New(keys, st, allOpts...)
	field = field.SetRect(textedit.Rect{W: 80, H: 1})
	return Model{
		field:   field,
		visible: false,
		styles:  st,
		keys:    keys,
	}
}

// WithHistoryLoader injects the function used to load search history from the
// store. Called by the workspace after StoreReadyMsg is received.
func (m Model) WithHistoryLoader(f func() ([]string, error)) Model {
	m.historyLoader = f
	return m
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
	m.hist = historyNav{} // force fresh DB load on next navigation
	m.draft = ""
	m.status = ""
	m.undoStack = m.undoStack[:0] // reset undo stack; keep backing array
	m.field = m.field.SetContent("")
	m.field = m.field.SetFocused(true)
	return m
}

// Close hides the bar.
func (m Model) Close() Model {
	m.visible = false
	m.hist = historyNav{} // stale after close; next Open forces fresh load
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
	case historyReadyMsg:
		return m.applyHistoryReady(msg), nil

	case tea.ClipboardMsg:
		if !m.field.Focused() {
			return m, nil
		}
		prevQuery := m.Query()
		var cmd tea.Cmd
		m.field, cmd = m.field.Update(msg)
		if q := m.Query(); q != prevQuery {
			m.undoStack = append(m.undoStack, prevQuery)
			m.hist = historyNav{}
			m.draft = q
		}
		return m, cmd

	case tea.PasteMsg:
		if !m.field.Focused() {
			return m, nil
		}
		prevQuery := m.Query()
		var cmd tea.Cmd
		m.field, cmd = m.field.Update(msg)
		if q := m.Query(); q != prevQuery {
			m.undoStack = append(m.undoStack, prevQuery)
			m.hist = historyNav{}
			m.draft = q
		}
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
		m.hist = historyNav{} // force fresh DB load for next navigation session
		m.draft = q
		return m, func() tea.Msg { return SubmitMsg{Query: q, Backward: true} }
	}

	// Enter → submit forward
	if msg.Code == tea.KeyEnter && msg.Mod == 0 {
		q := m.Query()
		m.hist = historyNav{} // force fresh DB load for next navigation session
		m.draft = q
		return m, func() tea.Msg { return SubmitMsg{Query: q, Backward: false} }
	}

	// Up → history: walk toward most-recent (then toward oldest)
	if msg.Code == tea.KeyUp && msg.Mod == 0 {
		return m.historyUp()
	}

	// Down → history: walk toward oldest (then back toward draft)
	if msg.Code == tea.KeyDown && msg.Mod == 0 {
		return m.historyDown()
	}

	// Undo → restore previous query
	if key.Matches(msg, m.keys.Undo) {
		return m.undo(), nil
	}

	// Any other key → forward to field and exit history navigation
	prevQuery := m.Query()
	var cmd tea.Cmd
	m.field, cmd = m.field.Update(msg)
	if q := m.Query(); q != prevQuery {
		m.undoStack = append(m.undoStack, prevQuery)
		m.hist = historyNav{}
		m.draft = q
	}
	return m, cmd
}

// ---- History navigation state machine ----

// historyUp moves toward the most-recent end on the first press.
// When history isn't loaded yet, emits an async Cmd to load from DB.
func (m Model) historyUp() (Model, tea.Cmd) {
	if !m.hist.browsing {
		if !m.hist.loaded {
			if m.historyLoader == nil {
				return m, nil
			}
			loader, draft := m.historyLoader, m.draft
			return m, func() tea.Msg {
				entries, err := loader()
				if err != nil || len(entries) == 0 {
					return nil
				}
				return historyReadyMsg{entries: entries, draft: draft, dir: 1}
			}
		}
		if len(m.hist.entries) == 0 {
			return m, nil
		}
		m.hist.browsing = true
		m.hist.idx = 0
	} else if m.hist.idx < len(m.hist.entries)-1 {
		m.hist.idx++
	}
	m.field = m.field.SetContent(m.hist.entries[m.hist.idx])
	return m, nil
}

// historyDown enters at the least-recent end on the first press.
// When history isn't loaded yet, emits an async Cmd to load from DB.
// Down past the most-recent end returns to the draft.
func (m Model) historyDown() (Model, tea.Cmd) {
	if !m.hist.browsing {
		if !m.hist.loaded {
			if m.historyLoader == nil {
				return m, nil
			}
			loader, draft := m.historyLoader, m.draft
			return m, func() tea.Msg {
				entries, err := loader()
				if err != nil || len(entries) == 0 {
					return nil
				}
				return historyReadyMsg{entries: entries, draft: draft, dir: -1}
			}
		}
		if len(m.hist.entries) == 0 {
			return m, nil
		}
		m.hist.browsing = true
		m.hist.idx = len(m.hist.entries) - 1
	} else {
		m.hist.idx--
		if m.hist.idx < 0 {
			m.hist.browsing = false
			m.hist.idx = 0
			m.field = m.field.SetContent(m.draft)
			return m, nil
		}
	}
	m.field = m.field.SetContent(m.hist.entries[m.hist.idx])
	return m, nil
}

// applyHistoryReady processes the async DB load result and navigates into it.
func (m Model) applyHistoryReady(msg historyReadyMsg) Model {
	if m.hist.loaded {
		return m // stale duplicate; already have results
	}
	ws := filterHistory(msg.entries, msg.draft)
	m.hist.entries = ws
	// loaded mirrors the pre-refactor entries!=nil gate exactly (byte-identical,
	// §1.7): a filter that removes every entry leaves loaded false, so the next
	// Up/Down press re-issues the DB load — same as before this struct existed.
	m.hist.loaded = ws != nil
	if len(ws) == 0 {
		return m
	}
	m.hist.browsing = true
	if msg.dir > 0 {
		m.hist.idx = 0
	} else {
		m.hist.idx = len(ws) - 1
	}
	m.field = m.field.SetContent(m.hist.entries[m.hist.idx])
	return m
}

// filterHistory returns entries filtered by subsequence match on draft.
// When draft is empty, all entries are returned.
func filterHistory(entries []string, draft string) []string {
	if draft == "" {
		return entries
	}
	var ws []string
	for _, h := range entries {
		if edSearch.FuzzyMatch(draft, h) {
			ws = append(ws, h)
		}
	}
	return ws
}

// ---- Undo ----

// undo restores the previous query from the undo stack.
func (m Model) undo() Model {
	if len(m.undoStack) == 0 {
		return m
	}
	prev := m.undoStack[len(m.undoStack)-1]
	m.undoStack = m.undoStack[:len(m.undoStack)-1]
	m.field = m.field.SetContent(prev)
	m.hist = historyNav{}
	m.draft = prev
	return m
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

// StatusFor builds a status string from match count values.
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
