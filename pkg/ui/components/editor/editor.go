package editor

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
	"rune/pkg/editor/history"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/breadcrumb"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

type ViewportState struct {
	TopRow    int
	ScrollCol int
}

type IndentConfig struct {
	UseTabs bool
	TabSize int
}

type SaveIdentity struct {
	Path        string
	RequestID   string
	ContentHash string
	InFlight    bool
}

type CursorInfo struct {
	Line         int
	Col          int
	WordCount    int
	Dirty        bool
	ChordPending string
}

type ClipboardPort struct {
	ReadText  func() (string, error)
	WriteText func(string) error
}

type Model struct {
	buf              buffer.Buffer
	cursors          cursor.CursorSet
	history          history.UndoStack
	dirty            bool
	savedContentHash string
	activeSave       SaveIdentity
	filePath         string
	softWrap         bool
	indent           IndentConfig

	syntaxMap  display.SyntaxMap
	wrapMap    display.WrapMap
	snapshot   display.DisplaySnapshot
	syntaxSnap display.SyntaxSnapshot
	wrapSnap   display.WrapSnapshot

	resolver keybind.Resolver
	registry command.Registry

	highlighter CodeHighlighter
	clipboard   ClipboardPort

	termCaps    terminal.TermCaps
	imageConfig ImageConfig
	mouse       mouseState
	findOverlay FindOverlay
	viewport    ViewportState
	breadcrumb  breadcrumb.Model
	keys        keymap.Bindings
	styles      styles.Styles
	width       int
	height      int
	focused     bool
}

func New(keys keymap.Bindings, st styles.Styles, reg command.Registry, resolver keybind.Resolver, caps terminal.TermCaps) Model {
	return Model{
		buf:         buffer.New(""),
		cursors:     cursor.CursorSet{},
		history:     history.New(time.Now),
		resolver:    resolver,
		registry:    reg,
		highlighter: ChromaHighlighter(),
		termCaps:    caps,
		imageConfig: ImageConfig{AssetsDir: "assets"},
		breadcrumb:  breadcrumb.New(st),
		keys:        keys,
		styles:      st,
	}
}

func (m Model) Init() tea.Cmd { return nil }

func hashContent(content string) string {
	sum := sha256.Sum256([]byte(content))
	return hex.EncodeToString(sum[:])
}

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	var cmd tea.Cmd
	switch msg := msg.(type) {
	case ClipboardContentMsg:
		if len(msg.ImageData) > 0 {
			return m.handleImagePaste(msg.ImageData, msg.MIMEType, time.Now())
		}
		return m.handlePasteContent(msg.Text, time.Now())

	case ImageSavedMsg:
		return m.handleImageSaved(msg.RelativePath, time.Now())

	case ImageSaveErrorMsg:
		// Error saved; no-op for now (could surface to footer)
		return m, nil

	case FileLoadedMsg:
		b, err := buffer.FromBytes(msg.Content)
		if err == nil {
			m.buf = b
			m.filePath = msg.Path
			m.cursors = cursor.CursorSet{} // simplified
			m.savedContentHash = hashContent(m.buf.Content())
			m.dirty = false
			m = m.syncDisplay()
		}

	case FileClosedMsg:
		if msg.Path == m.filePath {
			m.filePath = ""
			m.buf = buffer.New("")
			m.savedContentHash = ""
			m.dirty = false
			m = m.syncDisplay()
		}

	case FileSavedMsg:
		if m.filePath == msg.Path && m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
			m.activeSave.InFlight = false
			m.savedContentHash = msg.SavedContentHash
			if hashContent(m.buf.Content()) == msg.SavedContentHash {
				m.dirty = false
			}
		}

	case FileSaveErrorMsg:
		if m.filePath == msg.Path && m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
			m.activeSave.InFlight = false
			// handle error if needed
		}

	case tea.MouseClickMsg:
		return m.handleMouseClick(msg, time.Now())

	case tea.MouseMotionMsg:
		return m.handleMouseMotion(msg)

	case tea.MouseWheelMsg:
		return m.handleMouseWheel(msg)

	case tea.KeyPressMsg:
		if !m.focused {
			return m, nil
		}

		// Find overlay open commands (Cmd+F, Cmd+H) work regardless of overlay state
		if msg.Code == 'f' && msg.Mod == tea.ModMeta {
			m.findOverlay = m.findOverlay.open(false)
			return m, nil
		}
		if msg.Code == 'h' && msg.Mod == tea.ModMeta {
			m.findOverlay = m.findOverlay.open(true)
			return m, nil
		}

		// When find overlay is visible, it consumes ALL keys
		if m.findOverlay.visible {
			var consumed bool
			m.findOverlay, consumed = m.findOverlay.consumeKey(msg)
			if consumed {
				return m, nil
			}
		}

		// Undo: Cmd+Z (no resolver binding)
		if msg.Code == 'z' && msg.Mod == tea.ModMeta {
			m, cmd = m.applyUndo()
			m = m.syncDisplay()
			return m, cmd
		}

		// Redo: Cmd+Shift+Z (no resolver binding)
		if msg.Code == 'z' && msg.Mod == (tea.ModMeta|tea.ModShift) {
			m, cmd = m.applyRedo()
			m = m.syncDisplay()
			return m, cmd
		}

		// PrimaryAction: Enter key routes directly to edit.newline (no resolver binding)
		if msg.Code == tea.KeyEnter && msg.Mod == 0 {
			ctx := command.CommandContext{
				Buffer:  m.buf,
				Cursors: m.cursors,
			}
			res := m.registry.Execute("edit.newline", ctx)
			if res.Err == nil {
				return m.dispatchOperation(res, "edit.newline", time.Now())
			}
		}

		// Cancel: Escape key routes to multicursor.escape (no resolver binding)
		if msg.Code == tea.KeyEscape && msg.Mod == 0 {
			ctx := command.CommandContext{
				Buffer:  m.buf,
				Cursors: m.cursors,
			}
			res := m.registry.Execute("multicursor.escape", ctx)
			if res.Err == nil && res.Operation.Kind != command.OperationNone {
				return m.dispatchOperation(res, "multicursor.escape", time.Now())
			}
		}

		// Resolve via keybind resolver for all other keys
		chord := keybind.ChordFromKeyMsg(msg)
		hasSel := false
		for _, c := range m.cursors.All() {
			if c.HasSelection() {
				hasSel = true
				break
			}
		}
		resCtx := keybind.ResolverContext{
			EditorFocused:  true,
			HasSelection:   hasSel,
			HasMultiCursor: m.cursors.IsMulti(),
			ReadOnly:       false,
		}
		newResolver, resResult := m.resolver.Resolve(chord, resCtx)
		m.resolver = newResolver
		switch resResult.Kind {
		case keybind.ResultFound:
			res := m.registry.Execute(resResult.Command, command.CommandContext{
				Buffer:       m.buf,
				FilePath:     m.filePath,
				NewRequestID: func() string { return "req-time-" + time.Now().String() },
				HashContent:  hashContent,
			})
			m, cmd = m.dispatchOperation(res, resResult.Command, time.Now())
		case keybind.ResultMoreChordsNeeded:
			// Chord incomplete — wait for next key
		case keybind.ResultNoMatch:
			if msg.Mod == 0 && isPrintableChar(msg.Code) {
				char := string(msg.Code)
				res := m.registry.Execute("edit.insert-character", command.CommandContext{
					Buffer:  m.buf,
					Cursors: m.cursors,
					Args:    map[string]any{"char": char},
				})
				if res.Err == nil && res.Operation.Kind != command.OperationNone {
					m = m.applyOperation(res.Operation, history.EditInsertChar, time.Now())
					m = m.syncDisplay()
				}
			}
		}
	}
	return m, cmd
}

func (m Model) contentHeight() int {
	h := m.height - m.breadcrumb.Height()
	if h < 1 {
		return 1
	}
	return h
}

func (m Model) View() string {
	if m.width == 0 || m.height == 0 {
		return ""
	}

	bcView := m.breadcrumb.View()
	contentHeight := m.contentHeight()

	lines := m.snapshot.Slice(m.viewport.TopRow, contentHeight)
	lines = m.snapshot.SliceH(lines, m.viewport.ScrollCol, m.width)

	var renderedLines []string
	for _, l := range lines {
		var lineStr strings.Builder
		for _, sp := range l.Spans {
			lineStr.WriteString(m.renderSpan(sp))
		}
		renderedLines = append(renderedLines, lineStr.String())
	}

	for len(renderedLines) < contentHeight {
		renderedLines = append(renderedLines, "~")
	}

	content := strings.Join(renderedLines, "\n")

	ret := lipgloss.JoinVertical(lipgloss.Left, bcView, content)

	if !m.focused {
		ret = lipgloss.NewStyle().Faint(true).Render(ret)
	}

	return lipgloss.NewStyle().
		MaxWidth(m.width).
		MaxHeight(m.height).
		Width(m.width).
		Height(m.height).
		Render(ret)
}

func (m Model) SetSize(w, h int) Model {
	m.width = w
	m.height = h
	m.breadcrumb = m.breadcrumb.SetSize(w, 1)
	return m.syncDisplay()
}

func (m Model) Height() int             { return m.height }
func (m Model) SetFocused(f bool) Model { m.focused = f; return m }
func (m Model) Content() string         { return m.buf.Content() }
func (m Model) IsDirty() bool           { return m.dirty }
func (m Model) FilePath() string        { return m.filePath }
func (m Model) WantsModalInput() bool   { return m.findOverlay.visible }
func (m Model) StartSave() (Model, SaveIdentity, tea.Cmd) {
	req := SaveRequest{
		Path:        m.filePath,
		Content:     m.buf.Content(),
		RequestID:   fmt.Sprintf("req-%v", time.Now().UnixNano()),
		ContentHash: hashContent(m.buf.Content()),
	}
	return m.startSaveRequest(req)
}
func (m Model) CursorInfo() CursorInfo {
	return CursorInfo{Dirty: m.dirty}
}

func (m Model) SetContent(path string, content []byte) Model {
	b, err := buffer.FromBytes(content)
	if err == nil {
		m.buf = b
		m.filePath = path
		m.savedContentHash = hashContent(string(content))
		m.dirty = false
		m = m.syncDisplay()
	}
	return m
}

func (m Model) SetDir(dir string) Model {
	m.breadcrumb = m.breadcrumb.SetDir(dir)
	return m
}
func (m Model) OpenPath() string { return m.filePath }

// SetHighlighter replaces the code highlighter adapter. Used for testing.
func (m Model) SetHighlighter(h CodeHighlighter) Model { m.highlighter = h; return m }

// SetDirtyForTest marks the editor dirty without modifying content. Test-only.
func (m Model) SetDirtyForTest() Model { m.dirty = true; return m }

func PreferredWidth() int { return 40 }

func (m Model) SetClipboard(port ClipboardPort) Model {
	m.clipboard = port
	return m
}

// isPrintableChar reports whether the rune is a printable ASCII character.
func isPrintableChar(r rune) bool {
	return r >= ' ' && r <= '~'
}
