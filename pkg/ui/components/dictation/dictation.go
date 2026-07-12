package dictation

import (
	"context"
	"strings"

	tea "charm.land/bubbletea/v2"

	dictengine "rune/pkg/dictation"
	"rune/pkg/whisper"
)

// DoneMsg is emitted when a dictation session ends (final transcription or fatal error).
type DoneMsg struct{ Text string }

type pendingEdit struct {
	start, end int
	text       string
}

// engine groups the running dictation engine's handles: the context cancel
// that stops it and the message channel it delivers on (§6.3 cancellation
// pattern). Grouping them replaces the three scattered "cancel != nil →
// cancel() → cancel = nil" dances with one stop() chokepoint, and makes
// "is the engine delivering messages" a single named query (active()).
type engine struct {
	cancel context.CancelFunc
	ch     <-chan tea.Msg
}

// active reports whether the engine is delivering messages (a listen can be
// scheduled on ch).
func (e engine) active() bool { return e.ch != nil }

// stop cancels the engine's context, idempotently. The channel is
// deliberately NOT cleared here: a canceled engine may still deliver a
// FinalTranscriptionMsg (see Disable) — callers that also want to stop
// listening clear ch themselves.
func (e engine) stop() engine {
	if e.cancel != nil {
		e.cancel()
		e.cancel = nil
	}
	return e
}

// Model manages the dictation engine and the current session anchor.
// Non-rendering: no View method.
type Model struct {
	// Session state
	startOff   int  // byte offset where the current session started
	appliedLen int  // byte length of the last-applied chunk
	enabled    bool // true while a session is anchored

	// ticketDocID/ticketEpoch are the page's view-targeted result ticket
	// (Part IV), captured at Enable and validated by the page BEFORE draining
	// a pending edit — plain scalars, never the page's viewTicket type
	// (components import no page types, §10). A session anchored on one
	// buffer must never have its chunk applied to a different one (or the
	// SAME doc after a later undo/load/resolve replaced the buffer
	// wholesale) — the scattered Disable() calls at every known transition
	// (workspace_edit.go/workspace_merge_fresh.go) are the fast, immediate
	// UI-feedback path; this ticket is the structural backstop that catches
	// any transition those calls miss.
	ticketDocID int64
	ticketEpoch uint64

	// Engine state (§6.3 cancellation pattern)
	eng engine

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

// Ticket returns the (docID, epoch) captured at Enable, for the page to
// validate a drained pending edit against before applying it (Part IV).
func (m Model) Ticket() (docID int64, epoch uint64) { return m.ticketDocID, m.ticketEpoch }

// Enable anchors a dictation session to a byte offset in the focused editor,
// tagged with the page's current view ticket (docID/epoch) so a later drain
// can be validated against it. Call before StartCmd.
func (m Model) Enable(startOff int, ticketDocID int64, ticketEpoch uint64) Model {
	m.startOff = startOff
	m.appliedLen = 0
	m.enabled = true
	m.hasPending = false
	m.ticketDocID = ticketDocID
	m.ticketEpoch = ticketEpoch
	return m
}

// Disable cancels the engine and clears the session.
// The engine channel may still deliver a FinalTranscriptionMsg — do NOT
// close or clear it here.
func (m Model) Disable() Model {
	m.eng = m.eng.stop()
	m.enabled = false
	m.hasPending = false
	return m
}

// StartCmd starts the dictation engine using the default whisper config.
// Returns an updated Model (stores cancel func) and the start command.
func (m Model) StartCmd() (Model, tea.Cmd) {
	m.eng = m.eng.stop()
	ctx, cancel := context.WithCancel(context.Background())
	m.eng.cancel = cancel
	// Language deliberately left "" — the engine resolves it from the active
	// keyboard input source inside dictengine.StartCmd, BEHIND the test-stub
	// seam, because that resolution is a main-thread-only TIS cgo call that
	// must never run in a hermetic test (see dictengine.Config.Language).
	cfg := dictengine.Config{
		Whisper: whisper.Client{BaseURL: "http://127.0.0.1:8080", InferencePath: "/inference"},
	}
	return m, dictengine.StartCmd(ctx, cfg)
}

// Update handles dictation engine messages, managing ListenCmd scheduling and
// producing pending edits from partial transcriptions.
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case dictengine.ReadyMsg:
		m.eng.ch = msg.Ch
		return m, dictengine.ListenCmd(m.eng.ch)

	case dictengine.PartialTranscriptionMsg:
		if m.enabled {
			// §1.3 / CLAUDE.md #24: An upstream reset (empty or whitespace-only
			// Accumulated) must never be applied as a destructive ReplaceRange —
			// it would erase committed text. Drop the update and keep the current
			// pending range so the next non-empty result lands correctly.
			if strings.TrimSpace(msg.Accumulated) == "" {
				if m.eng.active() {
					return m, dictengine.ListenCmd(m.eng.ch)
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
		if m.eng.active() {
			return m, dictengine.ListenCmd(m.eng.ch)
		}
		return m, nil

	case dictengine.FinalTranscriptionMsg:
		finalText := msg.Text
		// §1.3/CLAUDE.md #24: mirrors the PartialTranscriptionMsg clamp above
		// — an empty/whitespace-only Final is an upstream reset, not a user
		// deletion. Without this guard, a session that had already applied a
		// non-empty partial (appliedLen>0) followed by an empty Final erased
		// the committed dictated text via a destructive ReplaceRange with no
		// TrimSpace guard (confirmed via FuzzHumanSession cluster 18 v=1 —
		// the "empty-reset hazard" variant). Dropping the update here keeps
		// the last-applied (non-empty) partial's text exactly as committed;
		// the session still ends normally (enabled=false, DoneMsg emitted).
		if m.enabled && m.appliedLen > 0 && strings.TrimSpace(finalText) != "" {
			// Finalize with the exact last text already in the buffer
			m.pending = pendingEdit{
				start: m.startOff,
				end:   m.startOff + m.appliedLen,
				text:  finalText,
			}
			m.hasPending = true
		}
		m.enabled = false
		m.eng.ch = nil
		capturedText := finalText
		return m, func() tea.Msg { return DoneMsg{Text: capturedText} }

	case dictengine.ErrorMsg:
		if msg.Fatal {
			m.eng = m.eng.stop()
			m.eng.ch = nil
			m.enabled = false
			return m, func() tea.Msg { return DoneMsg{} }
		}
		if m.eng.active() {
			return m, dictengine.ListenCmd(m.eng.ch)
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
