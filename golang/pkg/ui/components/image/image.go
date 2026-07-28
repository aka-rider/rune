package image

import (
	"sync/atomic"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/imagekit"
	"rune/pkg/terminal"
	"rune/pkg/vfs"
)

type State int

const (
	PendingDecode State = iota
	PendingTransmit
	Live
	Failed
)

// genCounter mints a process-wide unique spawn generation for every image.Model
// (New stamps m.gen from it). UpdateMsg/ErrorMsg carry the spawning instance's
// Gen; the envelope guard in Update drops any result whose Gen no longer
// matches — so a same-path async result from a despawned-then-respawned
// instance (mtime replacement, §3 discoverNewImages) can never be applied to
// the new instance by mistake, even though Path alone would match.
var genCounter atomic.Uint64

func nextGen() uint64 {
	return genCounter.Add(1)
}

type Model struct {
	path     string
	absPath  string
	state    State
	err      error // set on transition to Failed; see Err()
	gen      uint64
	termCaps terminal.TermCaps
	id       uint32
	cols     int
	rows     int
	pxW      int
	pxH      int
	mtime    int64
	cellSize imagekit.CellSize
	fs       vfs.FS // filesystem for reading image bytes; always non-nil — New normalizes nil to vfs.Disk{} (§1.4.9)

	visibleRows int

	iterm2Slices []string

	expanded bool

	anim anim

	maxCols int
	maxRows int
}

// anim groups an animated (GIF) image's frame-playback state. Grouping it
// lets canTick() reason about "is this image eligible to tick" as one
// value instead of scattering the same conjunction across ArmTick and the
// frameTickMsg handler (§1.7).
type anim struct {
	animated   bool
	frameIDs   []uint32
	frameIdx   int
	frameCount int
	delays     []time.Duration
	loopCount  int
	loopsDone  int
	tickGen    int
	ticking    bool
}

// New constructs an image Model. fsys is normalized to vfs.Disk{} when nil so
// every image-byte read goes through a non-nil filesystem — an in-memory VFS
// (tests/fuzz) resolves the same files the workspace serves (§1.4.9).
func New(path, absPath string, id uint32, mtime int64, caps terminal.TermCaps, cs imagekit.CellSize, maxCols, maxRows int, fsys vfs.FS) Model {
	if fsys == nil {
		fsys = vfs.Disk{}
	}
	return Model{
		path:     path,
		absPath:  absPath,
		id:       id,
		mtime:    mtime,
		expanded: true,
		termCaps: caps,
		cellSize: cs,
		maxCols:  maxCols,
		maxRows:  maxRows,
		fs:       fsys,
		state:    PendingDecode,
		gen:      nextGen(),
	}
}

func (m Model) Init() tea.Cmd {
	return DecodeCmd(m)
}

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case UpdateMsg:
		if msg.Path != m.path || msg.Gen != m.gen {
			return m, nil // stale-async by construction: despawned/respawned instance, or a different path
		}
		return m.handleInner(msg.inner)

	case ErrorMsg:
		if msg.Path != m.path || msg.Gen != m.gen {
			return m, nil
		}
		m.err = msg.Err
		return m.transition(Failed)
	}
	return m, nil
}

// handleInner applies one unwrapped lifecycle message. Every case is guarded
// by the CURRENT state — an illegal (state, message) pair is a silent drop,
// safe by construction once the envelope's Gen has already matched: no other
// async result for this exact spawn can be in flight for a state the message
// doesn't belong to. Two pairs are deliberately accepted outside the normal
// "one message advances one edge" shape: Live+transmittedMsg is an idempotent
// ack (a duplicate in-flight transmit completing after the model already
// reached Live via another), and Live+encodedMsg stores refreshed iTerm2
// slices (an overlapping Resize retransmit landing after an earlier one
// already closed the PendingTransmit->Live edge) — both leave state alone.
func (m Model) handleInner(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case decodedMsg:
		if m.state != PendingDecode {
			return m, nil
		}
		m.cols = msg.cols
		m.rows = msg.rows
		m.pxW = msg.pxW
		m.pxH = msg.pxH
		m.mtime = msg.mtime

		if msg.animated && msg.frameCount > 1 && m.termCaps.SupportsKittyGraphics() {
			m.anim.animated = true
			m.anim.frameCount = msg.frameCount
			m.anim.delays = msg.delays
			m.anim.loopCount = msg.loopCount
			m.anim.frameIdx = 0
			m.anim.loopsDone = 0
		}
		return m.transition(PendingTransmit)

	case transmittedMsg:
		switch m.state {
		case PendingTransmit:
			return m.transition(Live)
		case Live:
			return m, nil // idempotent ack
		default:
			return m, nil // illegal: PendingDecode/Failed never dispatch a transmit
		}

	case encodedMsg:
		switch m.state {
		case PendingTransmit:
			m.iterm2Slices = msg.slices
			return m.transition(Live)
		case Live:
			m.iterm2Slices = msg.slices // refreshed retransmit slices
			return m, nil
		default:
			return m, nil
		}

	case frameTickMsg:
		if msg.gen != m.anim.tickGen || m.state != Live || !m.anim.animated {
			return m, nil
		}
		next := msg.next
		if next >= m.anim.frameCount {
			m.anim.loopsDone++
			next = 0
			if m.animationShouldStop() {
				m.anim.frameIdx = m.anim.frameCount - 1
				m.anim.ticking = false
				return m, nil
			}
		}
		m.anim.frameIdx = next

		if !m.canTick() {
			m.anim.ticking = false // arm tick on scroll in
			return m, nil
		}
		return m, m.scheduleFrame(m.anim.tickGen, m.anim.frameIdx+1, m.frameDelay())
	}
	return m, nil
}

// transition is the ONLY writer of m.state; enterState receives prev because
// the two edges into PendingTransmit differ (initial transmit dispatch from
// PendingDecode vs a Resize retransmit from Live, whose Cmd the caller sends
// separately via RetransmitCmd).
func (m Model) transition(next State) (Model, tea.Cmd) {
	prev := m.state
	m.state = next
	return m.enterState(prev)
}

// enterState runs the entry action for the state m.state was just set to.
func (m Model) enterState(prev State) (Model, tea.Cmd) {
	switch m.state {
	case PendingTransmit:
		if prev == Live {
			return m, nil // Resize retransmit — parent dispatches RetransmitCmd
		}
		if m.anim.animated && m.anim.frameCount > 1 {
			// frameIDs will be populated via SetFrameIDs; transmit is NOT
			// triggered here for animated images — the editor allocates frame
			// IDs first (SetFrameIDs) and drives the transmit from there.
			return m, nil
		}
		if m.termCaps.SupportsKittyGraphics() {
			return m, TransmitCmd(m)
		}
		return m, EncodeITerm2Cmd(m)

	case Live:
		// No animation self-arm here: when the async Live ack lands,
		// visibleRows still holds the projection from BEFORE this instance
		// decoded (usually 0 — pre-decode embeds reserve no rows), so an arm
		// from inside the instance would consult stale visibility and no-op.
		// The parent's updateImages funnels through syncImageViewState — the
		// single re-arm chokepoint — right after applying this message, with
		// visibility freshly projected from the row-expanded snapshot.
		return m, func() tea.Msg { return ReadyMsg{Path: m.path} }

	case Failed:
		m.anim.ticking = false // stop ticking; Height()/IsLive() already reflect Failed
	}
	return m, nil
}

// SetVisibleRows records how many of the image's rows are currently within the
// viewport. It gates animation ticking (ArmTick / frameTickMsg, via canTick)
// so offscreen animated images don't schedule frames. Called from
// markdownedit's updateImages/armImageTicks (image_integration.go) on every
// image message and layout change.
func (m Model) SetVisibleRows(count int) Model {
	m.visibleRows = count
	return m
}

// SetExpanded updates the image's expanded/collapsed display state and
// reports whether this call is the transition FROM expanded TO collapsed —
// the one edge markdownedit needs (it must clear the terminal to erase the
// now-stale expanded rows). Replaces the old expanded/wasExpanded field pair
// plus a separate WasCollapsed() query (§1.7): the transition is computed
// once, here, from the pre-call value, instead of the caller reconstructing
// it by comparing WasCollapsed() before and after the call.
func (m Model) SetExpanded(expanded bool) (Model, bool) {
	collapsed := m.expanded && !expanded
	m.expanded = expanded
	return m, collapsed
}

func (m Model) SetFrameIDs(ids []uint32) (Model, tea.Cmd) {
	m.anim.frameIDs = ids
	if m.anim.animated && len(ids) > 0 {
		return m, TransmitAnimationCmd(m)
	}
	return m, nil
}

func (m Model) IsLive() bool {
	return m.state == Live
}

func (m Model) Height() int {
	if m.state == PendingDecode || m.state == Failed || m.rows <= 0 {
		return 1
	}
	return m.rows
}

func (m Model) Cols() int {
	return m.cols
}

func (m Model) State() State {
	return m.state
}

// Err returns the error that drove the transition to Failed, or nil if the
// image never failed (or hasn't yet).
func (m Model) Err() error {
	return m.err
}

func (m Model) Mtime() int64 {
	return m.mtime
}

// Animation helpers

// canTick reports whether this image is currently eligible to have an
// animation frame scheduled: it must be an animated, live image with more
// than one frame, and on screen (expanded, with rows in the viewport). This
// is the single gate ArmTick and the frameTickMsg continuation both consult
// (§1.7) — previously the same conjunction was hand-duplicated at both
// sites and could drift.
func (m Model) canTick() bool {
	return m.anim.animated && m.state == Live && m.anim.frameCount > 1 &&
		m.visibleRows > 0 && m.expanded
}

func (m Model) ArmTick() (Model, tea.Cmd) {
	if !m.canTick() || m.anim.ticking || m.animationShouldStop() {
		return m, nil
	}
	m.anim.ticking = true
	m.anim.tickGen++
	return m, m.scheduleFrame(m.anim.tickGen, m.anim.frameIdx+1, m.frameDelay())
}

func (m Model) animationShouldStop() bool {
	switch {
	case m.anim.loopCount == 0:
		return false
	case m.anim.loopCount < 0:
		return m.anim.loopsDone >= 1
	default:
		return m.anim.loopsDone >= m.anim.loopCount
	}
}

func (m Model) frameDelay() time.Duration {
	if m.anim.frameIdx >= 0 && m.anim.frameIdx < len(m.anim.delays) {
		return m.anim.delays[m.anim.frameIdx]
	}
	return 100 * time.Millisecond
}

func (m Model) NeedsFrameIDs() bool {
	return m.state == PendingTransmit && m.anim.animated && len(m.anim.frameIDs) == 0
}

func (m Model) ID() uint32 {
	return m.id
}

func (m Model) AbsPath() string {
	return m.absPath
}

func (m Model) ITerm2Slices() []string {
	return m.iterm2Slices
}

func (m Model) FrameCount() int {
	return m.anim.frameCount
}

func (m Model) Resize(maxCols, maxRows int) (Model, bool) {
	// Write back unconditionally, even for a not-yet-decoded image: without
	// this, a resize that lands before decode completes was silently dropped
	// (the early return below), leaving maxCols/maxRows stuck at whatever
	// New() captured — a latent config/runtime split.
	m.maxCols = maxCols
	m.maxRows = maxRows
	if m.pxW <= 0 || m.pxH <= 0 {
		return m, false
	}
	cols, rows := imagekit.FitCells(m.pxW, m.pxH, maxCols, maxRows, m.cellSize)
	if cols == m.cols && rows == m.rows {
		return m, false
	}
	m.cols = cols
	m.rows = rows
	m.iterm2Slices = nil
	if m.state == Live {
		m, _ = m.transition(PendingTransmit)
	}
	return m, true
}

func (m Model) CurrentID() uint32 {
	if m.anim.animated && len(m.anim.frameIDs) > m.anim.frameIdx {
		return m.anim.frameIDs[m.anim.frameIdx]
	}
	return m.id
}
