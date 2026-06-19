package dictation

import (
	"context"
	"strings"

	tea "charm.land/bubbletea/v2"

	dictengine "rune/pkg/dictation"
	"rune/pkg/inputlang"
	"rune/pkg/whisper"
)

// DoneMsg is emitted when a dictation session ends (final transcription or fatal error).
type DoneMsg struct{ Text string }

type pendingEdit struct {
	start, end int
	text       string
}

// Model manages the dictation engine and the current session anchor.
// Non-rendering: no View method.
type Model struct {
	// Session state
	startOff   int  // byte offset where the current session started
	appliedLen int  // byte length of the last-applied chunk
	enabled    bool // true while a session is anchored

	// Engine state (§6.3 cancellation pattern)
	cancel context.CancelFunc
	dictCh <-chan tea.Msg

	// Pending buffer edit (drained by TakePendingEdit)
	hasPending bool
	pending    pendingEdit
}

// New creates a new (idle) dictation Model.
func New() Model { return Model{} }

// Init returns nil (no startup commands needed).
func (m Model) Init() tea.Cmd { return nil }

// Enabled reports whether a session is active.
func (m Model) Enabled() bool { return m.enabled }

// Enable anchors a dictation session to a byte offset in the focused editor.
// Call before StartCmd.
func (m Model) Enable(startOff int) Model {
	m.startOff = startOff
	m.appliedLen = 0
	m.enabled = true
	m.hasPending = false
	return m
}

// Disable cancels the engine and clears the session.
// The dictCh may still deliver a FinalTranscriptionMsg — do NOT close it here.
func (m Model) Disable() Model {
	if m.cancel != nil {
		m.cancel()
		m.cancel = nil
	}
	m.enabled = false
	m.hasPending = false
	return m
}

// StartCmd starts the dictation engine using the default whisper config.
// Returns an updated Model (stores cancel func) and the start command.
func (m Model) StartCmd() (Model, tea.Cmd) {
	if m.cancel != nil {
		m.cancel()
	}
	ctx, cancel := context.WithCancel(context.Background())
	m.cancel = cancel
	cfg := dictengine.Config{
		Whisper:  whisper.Client{BaseURL: "http://127.0.0.1:2022", InferencePath: "/v1/audio/transcriptions"},
		Language: inputlang.Current(),
	}
	return m, dictengine.StartCmd(ctx, cfg)
}

// Update handles dictation engine messages, managing ListenCmd scheduling and
// producing pending edits from partial transcriptions.
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case dictengine.ReadyMsg:
		m.dictCh = msg.Ch
		return m, dictengine.ListenCmd(m.dictCh)

	case dictengine.PartialTranscriptionMsg:
		if m.enabled {
			// §1.3 / CLAUDE.md #24: An upstream reset (empty or whitespace-only
			// Accumulated) must never be applied as a destructive ReplaceRange —
			// it would erase committed text. Drop the update and keep the current
			// pending range so the next non-empty result lands correctly.
			if strings.TrimSpace(msg.Accumulated) == "" {
				if m.dictCh != nil {
					return m, dictengine.ListenCmd(m.dictCh)
				}
				return m, nil
			}
			start := m.startOff
			end := m.startOff + m.appliedLen
			text := msg.Accumulated
			m.pending = pendingEdit{start: start, end: end, text: text}
			m.hasPending = true
			m.appliedLen = len(text)
		}
		if m.dictCh != nil {
			return m, dictengine.ListenCmd(m.dictCh)
		}
		return m, nil

	case dictengine.FinalTranscriptionMsg:
		finalText := msg.Text
		if m.enabled && m.appliedLen > 0 {
			// Finalize with the exact last text already in the buffer
			finalText = msg.Text
			m.pending = pendingEdit{
				start: m.startOff,
				end:   m.startOff + m.appliedLen,
				text:  finalText,
			}
			m.hasPending = true
		}
		m.enabled = false
		m.dictCh = nil
		capturedText := finalText
		return m, func() tea.Msg { return DoneMsg{Text: capturedText} }

	case dictengine.ErrorMsg:
		if msg.Fatal {
			if m.cancel != nil {
				m.cancel()
				m.cancel = nil
			}
			m.dictCh = nil
			m.enabled = false
			return m, func() tea.Msg { return DoneMsg{} }
		}
		if m.dictCh != nil {
			return m, dictengine.ListenCmd(m.dictCh)
		}
		return m, nil
	}
	return m, nil
}

// TakePendingEdit returns any pending buffer edit and clears it from the Model.
// Returns (updatedModel, start, end, text, ok). Call after Update.
func (m Model) TakePendingEdit() (Model, int, int, string, bool) {
	if !m.hasPending {
		return m, 0, 0, "", false
	}
	p := m.pending
	m.hasPending = false
	return m, p.start, p.end, p.text, true
}
