package image

import (
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/imagekit"
	"rune/pkg/terminal"
)

type State int

const (
	PendingDecode State = iota
	PendingTransmit
	Live
	Failed
)

type Model struct {
	path     string
	absPath  string
	state    State
	termCaps terminal.TermCaps
	id       uint32
	cols     int
	rows     int
	pxW      int
	pxH      int
	mtime    int64
	cellSize imagekit.CellSize

	visibleTop  int
	visibleRows int
	scrollCol   int
	maxWidth    int

	iterm2Slices    []string
	screenRow       int
	screenCol       int
	prevScreenRow   int
	prevVisibleRows int
	prevCol         int
	placed          bool

	expanded    bool
	wasExpanded bool

	animated   bool
	frameIDs   []uint32
	frameIdx   int
	frameCount int
	delays     []time.Duration
	loopCount  int
	loopsDone  int
	tickGen    int
	ticking    bool
	maxCols    int
	maxRows    int
}

func New(path, absPath string, id uint32, mtime int64, caps terminal.TermCaps, cs imagekit.CellSize, maxCols, maxRows int) Model {
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
		state:    PendingDecode,
	}
}

func (m Model) Init() tea.Cmd {
	return DecodeCmd(m)
}

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case UpdateMsg:
		if msg.Path != m.path {
			return m, nil
		}
		// Unwrap
		return m.handleInner(msg.inner)
	}
	return m, nil
}

func (m Model) handleInner(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case decodedMsg:
		m.cols = msg.cols
		m.rows = msg.rows
		m.pxW = msg.pxW
		m.pxH = msg.pxH
		m.mtime = msg.mtime
		m.state = PendingTransmit

		if msg.animated && msg.frameCount > 1 && m.termCaps.SupportsKittyGraphics() {
			m.animated = true
			m.frameCount = msg.frameCount
			m.delays = msg.delays
			m.loopCount = msg.loopCount
			m.frameIdx = 0
			m.loopsDone = 0
			// frameIDs will be populated via SetFrameIDs
			return m, func() tea.Msg {
				// We don't automatically trigger transmit here for animated images,
				// because frameIDs need to be allocated first by the editor.
				// Wait, the editor can't easily intercept after decode to allocate IDs
				// and *then* transmit as a side effect.
				// Let's emit a ReadyMsg equivalent but indicating needs-frame-IDs?
				// Actually, we can return a message to let the editor know decode is done.
				// Or wait, if we allocate frame IDs in imageIDAllocator, we can do it when the editor receives ReadyMsg? No, ReadyMsg means it's ready to draw.
				return nil
			}
		}

		if m.termCaps.SupportsKittyGraphics() {
			return m, TransmitCmd(m)
		}
		return m, EncodeITerm2Cmd(m)

	case transmittedMsg:
		m.state = Live
		var cmd tea.Cmd
		if m.animated && m.frameCount > 1 {
			m, cmd = m.ArmTick()
		}
		return m, tea.Batch(cmd, func() tea.Msg { return ReadyMsg{Path: m.path} })

	case encodedMsg:
		m.iterm2Slices = msg.slices
		m.state = Live
		return m, func() tea.Msg { return ReadyMsg{Path: m.path} }

	case frameTickMsg:
		if msg.gen != m.tickGen || m.state != Live || !m.animated {
			return m, nil
		}
		next := msg.next
		if next >= m.frameCount {
			m.loopsDone++
			next = 0
			if m.animationShouldStop() {
				m.frameIdx = m.frameCount - 1
				m.ticking = false
				return m, nil
			}
		}
		m.frameIdx = next

		if m.visibleRows == 0 || !m.expanded {
			m.ticking = false // arm tick on scroll in
			return m, nil
		}
		return m, m.scheduleFrame(m.tickGen, m.frameIdx+1, m.frameDelay())
	}
	return m, nil
}

func (m Model) SetVisibleRange(top, count, scrollCol, maxWidth int) Model {
	m.visibleTop = top
	m.visibleRows = count
	m.scrollCol = scrollCol
	m.maxWidth = maxWidth
	return m
}

func (m Model) SetScreenPosition(row, col int) Model {
	m.screenRow = row
	m.screenCol = col
	return m
}

func (m Model) SetExpanded(expanded bool) Model {
	m.wasExpanded = m.expanded
	m.expanded = expanded
	return m
}

func (m Model) WasCollapsed() bool {
	return m.wasExpanded && !m.expanded
}

func (m Model) SetFrameIDs(ids []uint32) (Model, tea.Cmd) {
	m.frameIDs = ids
	if m.animated && len(ids) > 0 {
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

func (m Model) Mtime() int64 {
	return m.mtime
}

func (m Model) Path() string {
	return m.path
}

// Animation helpers
func (m Model) ArmTick() (Model, tea.Cmd) {
	if !m.animated || m.state != Live || m.frameCount <= 1 {
		return m, nil
	}
	if m.ticking || m.animationShouldStop() || m.visibleRows == 0 || !m.expanded {
		return m, nil
	}
	m.ticking = true
	m.tickGen++
	return m, m.scheduleFrame(m.tickGen, m.frameIdx+1, m.frameDelay())
}

func (m Model) animationShouldStop() bool {
	switch {
	case m.loopCount == 0:
		return false
	case m.loopCount < 0:
		return m.loopsDone >= 1
	default:
		return m.loopsDone >= m.loopCount
	}
}

func (m Model) frameDelay() time.Duration {
	if m.frameIdx >= 0 && m.frameIdx < len(m.delays) {
		return m.delays[m.frameIdx]
	}
	return 100 * time.Millisecond
}

// MarkFailed manually sets the model to failed state, e.g. on error msg
func (m Model) MarkFailed() Model {
	m.state = Failed
	return m
}

func (m Model) NeedsFrameIDs() bool {
	return m.state == PendingTransmit && m.animated && len(m.frameIDs) == 0
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
	return m.frameCount
}

func (m Model) LiveIDs() []uint32 {
	if m.state != Live {
		return nil
	}
	ids := []uint32{m.id}
	if len(m.frameIDs) > 0 {
		ids = append(ids, m.frameIDs...)
	}
	return ids
}

func (m Model) Resize(maxCols, maxRows int) (Model, bool) {
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
		m.state = PendingTransmit
	}
	return m, true
}

func (m Model) CurrentID() uint32 {
	if m.animated && len(m.frameIDs) > m.frameIdx {
		return m.frameIDs[m.frameIdx]
	}
	return m.id
}
