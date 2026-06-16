package markdownedit

import (
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
	"rune/pkg/editor/keybind"
	"rune/pkg/imagekit"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/image"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// Model is the markdown editing component. It embeds textedit.Model for all
// base text-editing behavior and adds image rendering, link handling, and
// markdown-specific cell styling.
type Model struct {
	textedit.Model

	highlighter CodeHighlighter
	termCaps    terminal.TermCaps
	imageConfig ImageConfig
	images      map[string]image.Model
	idAlloc     imageIDAllocator
	cellSize    imagekit.CellSize
	mouse       mouseState

	lastPlacementSeq    string
	pendingPlacementSeq string

	dir    string        // base directory for image/link resolution
	styles styles.Styles // cached for cell rendering
}

// Option configures a markdownedit Model during construction.
// The underlying type is textedit.Option so options can be composed across both packages.
type Option = textedit.Option

// WithRegistry sets the command registry on the underlying textedit.
func WithRegistry(reg command.Registry) Option {
	return textedit.WithRegistry(reg)
}

// WithResolver sets the keybind resolver on the underlying textedit.
func WithResolver(resolver keybind.Resolver) Option {
	return textedit.WithResolver(resolver)
}

// New creates a new markdownedit Model.
func New(keys keymap.Bindings, st styles.Styles, caps terminal.TermCaps, opts ...Option) Model {
	allOpts := append([]textedit.Option{textedit.WithSyncFunc(textedit.PlainSync)}, opts...)
	base := textedit.New(keys, st, allOpts...)
	return Model{
		Model:       base,
		highlighter: ChromaHighlighter(),
		termCaps:    caps,
		imageConfig: ImageConfig{AssetsDir: "assets"},
		images:      map[string]image.Model{},
		idAlloc:     newImageIDAllocator(),
		cellSize:    imagekit.DefaultCellSize(),
		styles:      st,
	}
}

func (m Model) Init() tea.Cmd { return m.Model.Init() }

// SetFocused shadows textedit.Model.SetFocused to return markdownedit.Model.
func (m Model) SetFocused(f bool) Model {
	m.Model = m.Model.SetFocused(f)
	return m
}

// SetReadOnly shadows textedit.Model.SetReadOnly to return markdownedit.Model.
func (m Model) SetReadOnly(ro bool) Model {
	m.Model = m.Model.SetReadOnly(ro)
	return m
}

// GotoBottom shadows textedit.Model.GotoBottom to return markdownedit.Model.
func (m Model) GotoBottom() Model {
	m.Model = m.Model.GotoBottom()
	return m
}

// DrainEdits shadows textedit.Model.DrainEdits to return markdownedit.Model.
func (m Model) DrainEdits() (Model, []buffer.AppliedEdit) {
	var edits []buffer.AppliedEdit
	m.Model, edits = m.Model.DrainEdits()
	return m, edits
}

// SetCursors shadows textedit.Model.SetCursors to return markdownedit.Model.
func (m Model) SetCursors(cs []cursor.Cursor) Model {
	m.Model = m.Model.SetCursors(cs)
	return m
}

// ApplyInverse shadows textedit.Model.ApplyInverse, also running afterContentChange.
func (m Model) ApplyInverse(edits []buffer.AppliedEdit) (Model, tea.Cmd) {
	m.Model = m.Model.ApplyInverse(edits)
	return m.afterContentChange()
}

// Reapply shadows textedit.Model.Reapply, also running afterContentChange.
func (m Model) Reapply(edits []buffer.AppliedEdit) (Model, tea.Cmd) {
	m.Model = m.Model.Reapply(edits)
	return m.afterContentChange()
}

// Update is the outermost wrapper: routes the message, then emits inline
// image placements (change-gated). Per D7.
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	m, cmd := m.routeUpdate(msg)
	var pcmd tea.Cmd
	m, pcmd = m.emitImagePlacements()
	return m, tea.Batch(cmd, pcmd)
}

func (m Model) routeUpdate(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case textedit.ClipboardContentMsg:
		if len(msg.ImageData) > 0 {
			return m.handleImagePaste(msg.ImageData, msg.MIMEType, time.Now())
		}
		// Text paste: delegate to textedit, then afterContentChange
		var cmd tea.Cmd
		m.Model, cmd = m.Model.Update(msg)
		m, aCmd := m.afterContentChange()
		return m, tea.Batch(cmd, aCmd)

	case ImageSavedMsg:
		return m.handleImageSaved(msg.RelativePath, time.Now())

	case ImageSaveErrorMsg:
		return m, nil

	case image.UpdateMsg, image.ReadyMsg, image.ErrorMsg:
		return m.updateImages(msg)

	case placementTickMsg:
		return m.handlePlacementTick()

	case tea.MouseClickMsg:
		return m.handleMouseClick(msg, time.Now())

	case tea.MouseMotionMsg:
		return m.handleMouseMotion(msg)

	case tea.MouseWheelMsg:
		return m.handleMouseWheel(msg)

	default:
		// All other messages (keys, etc.) go to textedit, then afterContentChange.
		var cmd tea.Cmd
		prevRev := m.Model.Revision()
		m.Model, cmd = m.Model.Update(msg)

		// Only run afterContentChange if the buffer was mutated.
		if m.Model.Revision() != prevRev {
			m, aCmd := m.afterContentChange()
			return m, tea.Batch(cmd, aCmd)
		}
		return m, cmd
	}
}

// afterContentChange expands image rows after textedit's syncDisplay and
// re-discovers embedded images. Called whenever buffer content changes.
func (m Model) afterContentChange() (Model, tea.Cmd) {
	snap := m.Model.Snapshot()
	snap = display.ExpandImageRows(snap, m.imageDimsFor)
	m.Model = m.Model.SetSnapshot(snap)
	m.Model = m.Model.ScrollToCursor()

	var cmds []tea.Cmd

	m, collapsed := m.detectImageCollapse()
	if collapsed {
		cmds = append(cmds, tea.ClearScreen)
	}

	var dcmd tea.Cmd
	m, dcmd = m.discoverNewImages()
	if dcmd != nil {
		cmds = append(cmds, dcmd)
	}

	return m, tea.Batch(cmds...)
}

// SetRect sets position and size. Overrides textedit.SetRect to also resize images.
func (m Model) SetRect(r textedit.Rect) Model {
	m.Model = m.Model.SetRect(r)
	maxCols := m.Model.ImageMaxCols()
	maxRows := m.Model.ContentHeight()
	m = m.resizeImages(maxCols, maxRows)
	return m
}

// SetContent replaces buffer content and expands image rows immediately.
func (m Model) SetContent(content string) Model {
	m.Model = m.Model.SetContent(content)
	snap := m.Model.Snapshot()
	snap = display.ExpandImageRows(snap, m.imageDimsFor)
	m.Model = m.Model.SetSnapshot(snap)
	m.Model = m.Model.ScrollToCursor()
	return m
}

// SetDir sets the base directory for resolving relative image embeds and links.
func (m Model) SetDir(dir string) Model {
	m.dir = dir
	return m
}

// ReplaceRange replaces the range [start, end) with text and runs afterContentChange.
func (m Model) ReplaceRange(start, end int, text string) (Model, tea.Cmd) {
	m.Model = m.Model.ReplaceRange(start, end, text)
	return m.afterContentChange()
}

// AppendText appends text at the primary cursor position and runs afterContentChange.
func (m Model) AppendText(text string) (Model, tea.Cmd) {
	m.Model = m.Model.AppendText(text)
	return m.afterContentChange()
}

// ApplyMergeResult applies merged content from a 3-way merge operation.
func (m Model) ApplyMergeResult(content string) (Model, tea.Cmd) {
	return m.ReplaceRange(0, len(m.Model.Content()), content)
}
