package editor

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"path/filepath"
	"strings"
	"time"
	"unicode/utf8"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
	"rune/pkg/editor/history"
	"rune/pkg/editor/keybind"
	"rune/pkg/imagekit"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/breadcrumb"
	"rune/pkg/ui/components/editor/title"
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

type dictationState struct {
	active   bool
	startOff int // byte offset where dictation text begins
	totalLen int // byte length of all dictation text currently in buf
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

	termCaps    terminal.TermCaps
	imageConfig ImageConfig
	images      imageRegistry
	cellSize    imagekit.CellSize
	mouse       mouseState
	findOverlay FindOverlay
	dictation   dictationState
	viewport    ViewportState
	title       title.Model
	breadcrumb  breadcrumb.Model
	keys        keymap.Bindings
	styles      styles.Styles
	width       int
	height      int
	offsetX     int
	offsetY     int
	focused     bool

	lastPlacementSeq string // last inline-image escape sequence emitted via tea.Raw (change-gate)
}

func New(keys keymap.Bindings, st styles.Styles, reg command.Registry, resolver keybind.Resolver, caps terminal.TermCaps) Model {
	return Model{
		buf:         buffer.New(""),
		cursors:     cursor.NewCursorSet(0),
		history:     history.New(time.Now),
		resolver:    resolver,
		registry:    reg,
		highlighter: ChromaHighlighter(),
		termCaps:    caps,
		imageConfig: ImageConfig{AssetsDir: "assets"},
		images:      newImageRegistry(),
		cellSize:    imagekit.DefaultCellSize(),
		title:       title.New("Untitled", st),
		breadcrumb:  breadcrumb.New(st, validateFilename),
		keys:        keys,
		styles:      st,
	}
}

func (m Model) Init() tea.Cmd { return nil }

func hashContent(content string) string {
	sum := sha256.Sum256([]byte(content))
	return hex.EncodeToString(sum[:])
}

// Update handles a message and then emits any inline-image placement escapes
// via tea.Raw (the placement is gated so it only re-emits when the visible
// placement set changes). The message handling proper lives in update.
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	m, cmd := m.update(msg)
	var pcmd tea.Cmd
	m, pcmd = m.emitInlinePlacements()
	if pcmd != nil {
		cmd = tea.Batch(cmd, pcmd)
	}
	return m, cmd
}

func (m Model) update(msg tea.Msg) (Model, tea.Cmd) {
	var cmd tea.Cmd
	switch msg := msg.(type) {
	case ClipboardContentMsg:
		if len(msg.ImageData) > 0 {
			return m.handleImagePaste(msg.ImageData, msg.MIMEType, time.Now())
		}
		return m.handlePasteContent(msg.Text, time.Now())

	case tea.ClipboardMsg:
		return m.handlePasteContent(msg.Content, time.Now())

	case tea.PasteMsg:
		return m.handlePasteContent(msg.Content, time.Now())

	case ImageSavedMsg:
		return m.handleImageSaved(msg.RelativePath, time.Now())

	case ImageSaveErrorMsg:
		// Error saved; no-op for now (could surface to footer)
		return m, nil

	case ImageDecodedMsg:
		return m.handleImageDecoded(msg)

	case ImageTransmittedMsg:
		return m.handleImageTransmitted(msg)

	case ImageEncodedMsg:
		return m.handleImageEncoded(msg)

	case ImagePlacedMsg:
		// Placement acknowledged — no action needed.
		return m, nil

	case ImageDecodeErrorMsg:
		return m.handleImageError(msg.Path)

	case ImageTransmitErrorMsg:
		return m.handleImageError(msg.Path)

	case LinkClickedMsg:
		// Link click is handled by the workspace page which opens the file.
		// The editor just emits the message via a Cmd.
		return m, nil

	case imageFrameTickMsg:
		return m.handleImageFrameTick(msg)

	case tea.WindowSizeMsg:
		// The workspace sizes children (SetSize) before forwarding this, so the
		// registry's cell footprints are already updated; re-transmit images at
		// their new size and (re)arm animation ticks.
		var acmd tea.Cmd
		m, acmd = m.armImageTicks()
		return m, tea.Batch(m.retransmitImagesCmd(), acmd)

	case FileLoadedMsg:
		b, err := buffer.FromBytes(msg.Content)
		if err == nil {
			m, cmd = m.clearImages() // delete any prior file's images
			cmds := []tea.Cmd{cmd}
			m.buf = b
			m.filePath = msg.Path
			m.breadcrumb = m.breadcrumb.SetPath(msg.Path)
			m.cursors = cursor.NewCursorSet(0)
			m.savedContentHash = hashContent(m.buf.Content())
			m.dirty = false
			// Set title from filename stem
			stem := strings.TrimSuffix(filepath.Base(msg.Path), filepath.Ext(msg.Path))
			m.title = m.title.SetText(stem)
			m.breadcrumb = m.breadcrumb.SetUntitledName(filepath.Base(msg.Path))
			m = m.syncDisplay()
			var dcmd tea.Cmd
			m, dcmd = m.discoverNewImages()
			cmds = append(cmds, dcmd)
			return m, tea.Batch(cmds...)
		}

	case FileClosedMsg:
		if msg.Path == m.filePath {
			m, cmd = m.clearImages()
			m.filePath = ""
			m.breadcrumb = m.breadcrumb.SetPath("") // Clear the path so it uses untitled fallback
			m.buf = buffer.New("")
			m.savedContentHash = ""
			m.dirty = false
			m = m.syncDisplay()
			return m, cmd
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

	case breadcrumb.SetPathMsg:
		m.breadcrumb, cmd = m.breadcrumb.Update(msg)
		return m, cmd

	case title.RenameRequestMsg:
		if m.filePath != "" {
			return m, FileRenameCmd(m.filePath, msg.Name)
		}
		// Untitled: bubble up to workspace for file creation
		name := msg.Name
		return m, func() tea.Msg { return UntitledRenameMsg{Name: name} }

	case title.FocusReturnMsg:
		m.title = m.title.SetFocused(false)
		// Place cursor at beginning of content
		m.cursors = cursor.NewCursorSet(0)
		m = m.scrollToCursor()
		return m, nil

	case FileRenamedMsg:
		if msg.OldPath == m.filePath {
			m.filePath = msg.NewPath
			m.breadcrumb = m.breadcrumb.SetPath(msg.NewPath)
			stem := strings.TrimSuffix(filepath.Base(msg.NewPath), filepath.Ext(msg.NewPath))
			m.title = m.title.SetText(stem)
			m.breadcrumb = m.breadcrumb.SetUntitledName(filepath.Base(msg.NewPath))
		}

	case FileRenameErrorMsg:
		// no-op for now

	case tea.KeyPressMsg:
		if !m.focused {
			return m, nil
		}

		// Title is focused — forward all keys to it.
		if m.title.Focused() {
			var tcmd tea.Cmd
			m.title, tcmd = m.title.Update(msg)
			return m, tcmd
		}

		// Find overlay open commands (Cmd+F, Cmd+H) work regardless of overlay state
		if msg.Code == 'f' && msg.Mod == tea.ModSuper {
			m.findOverlay = m.findOverlay.open(false)
			return m, nil
		}
		if msg.Code == 'h' && msg.Mod == tea.ModSuper {
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
		if msg.Code == 'z' && msg.Mod == tea.ModSuper {
			m, cmd = m.applyUndo()
			m = m.syncDisplay()
			m = m.scrollToCursor()
			var ccmd tea.Cmd
			var collapsed bool
			m, collapsed = m.detectImageCollapse()
			if collapsed {
				ccmd = tea.ClearScreen
			}
			var dcmd tea.Cmd
			m, dcmd = m.discoverNewImages()
			return m, tea.Batch(cmd, dcmd, ccmd)
		}

		// Redo: Cmd+Shift+Z (no resolver binding)
		if msg.Code == 'z' && msg.Mod == (tea.ModSuper|tea.ModShift) {
			m, cmd = m.applyRedo()
			m = m.syncDisplay()
			m = m.scrollToCursor()
			var ccmd tea.Cmd
			var collapsed bool
			m, collapsed = m.detectImageCollapse()
			if collapsed {
				ccmd = tea.ClearScreen
			}
			var dcmd tea.Cmd
			m, dcmd = m.discoverNewImages()
			return m, tea.Batch(cmd, dcmd, ccmd)
		}

		// Cursor at top of buffer and pressing Up → focus title
		if msg.Code == tea.KeyUp && msg.Mod == 0 {
			// Check if all cursors are on the first visual row
			atTop := true
			for _, c := range m.cursors.All() {
				bp := m.buf.OffsetToLineCol(c.Position)
				sp := m.syntaxSnap.BufferToSyntax(bp)
				wp := m.wrapSnap.SyntaxToWrap(sp)
				if wp.Row > 0 {
					atTop = false
					break
				}
			}
			if atTop && m.viewport.TopRow == 0 {
				m.title = m.title.SetFocused(true)
				return m, nil
			}
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
			contentHeight := m.contentHeight()
			topRow := m.viewport.TopRow
			scrollCol := m.viewport.ScrollCol
			totalRows := m.snapshot.TotalRows
			res := m.registry.Execute(resResult.Command, command.CommandContext{
				Buffer:         m.buf,
				Cursors:        m.cursors,
				FilePath:       m.filePath,
				NewRequestID:   func() string { return "req-time-" + time.Now().String() },
				HashContent:    hashContent,
				BufferToSyntax: m.syntaxSnap.BufferToSyntax,
				SyntaxToBuffer: m.syntaxSnap.SyntaxToBuffer,
				SyntaxToWrap:   m.wrapSnap.SyntaxToWrap,
				WrapToSyntax:   m.wrapSnap.WrapToSyntax,
				WrapVisualCol:  m.wrapSnap.VisualCol,
				WrapByteCol:    m.wrapSnap.ByteColFromVisual,
				ViewportBounds: func() (int, int) { return topRow, topRow + contentHeight },
				ScrollCol:      func() int { return scrollCol },
				ViewportHeight: func() int { return contentHeight },
				TotalRows:      func() int { return totalRows },
			})
			m, cmd = m.dispatchOperation(res, resResult.Command, time.Now())
		case keybind.ResultMoreChordsNeeded:
			// Chord incomplete — wait for next key
		case keybind.ResultNoMatch:
			text := msg.Text
			if text == "" && msg.Mod == 0 && isPrintableChar(msg.Code) {
				text = string(msg.Code)
			}
			if text != "" {
				res := m.registry.Execute("edit.insert-character", command.CommandContext{
					Buffer:  m.buf,
					Cursors: m.cursors,
					Args:    map[string]any{"char": text},
				})
				if res.Err == nil && res.Operation.Kind != command.OperationNone {
					m = m.applyOperation(res.Operation, history.EditInsertChar, time.Now())
					m = m.syncDisplay()
					m = m.scrollToCursor()
					var ccmd tea.Cmd
					var collapsed bool
					m, collapsed = m.detectImageCollapse()
					if collapsed {
						ccmd = tea.ClearScreen
					}
					var dcmd tea.Cmd
					m, dcmd = m.discoverNewImages()
					cmd = tea.Batch(cmd, dcmd, ccmd)
				}
			}
		}
	}
	// Forward all messages to title sub-component (handles debounce timers).
	var tcmd tea.Cmd
	m.title, tcmd = m.title.Update(msg)
	if tcmd != nil {
		cmd = tea.Batch(cmd, tcmd)
	}
	return m, cmd
}

func (m Model) headerHeight() int {
	return m.title.Height()
}

func (m Model) contentHeight() int {
	h := m.height - m.headerHeight()
	if h < 1 {
		return 1
	}
	return h
}

// imageMaxCols returns the maximum column width for rendered images: the editor
// width minus 2 (1 char left margin + 1 char right margin) so images don't
// butt against borders. Clamped to at least 1.
func (m Model) imageMaxCols() int {
	w := m.width - 2
	if w < 1 {
		return 1
	}
	return w
}

func (m Model) SetSize(w, h int) Model {
	changed := w != m.width || h != m.height
	m.width = w
	m.height = h
	m.breadcrumb = m.breadcrumb.SetSize(w, 1)
	m.title = m.title.SetSize(w, 1)
	if changed {
		// Recompute image cell footprints for the new size (no I/O). The
		// re-transmit Cmd is emitted from the WindowSizeMsg arm, which the
		// workspace forwards after this SetSize runs.
		m = m.resizeImages(m.imageMaxCols(), m.contentHeight())
	}
	return m.syncDisplay()
}

func (m Model) SetOffset(x, y int) Model { m.offsetX = x; m.offsetY = y; return m }

func (m Model) Height() int { return m.height }
func (m Model) SetFocused(f bool) Model {
	changed := m.focused != f
	if !f {
		m.title = m.title.SetFocused(false)
	}
	m.focused = f
	// Resync display when focus changes because reveal decisions depend on it.
	if changed && m.buf.Content() != "" {
		m = m.syncDisplay()
	}
	return m
}
func (m Model) Content() string          { return m.buf.Content() }
func (m Model) IsDirty() bool            { return m.dirty }
func (m Model) FilePath() string         { return m.filePath }
func (m Model) WantsModalInput() bool    { return m.findOverlay.visible }
func (m Model) TitleText() string        { return m.title.Text() }
func (m Model) TitleIsPlaceholder() bool { return m.title.IsPlaceholder() }

func (m Model) SetTitle(name string) Model {
	m.title = m.title.SetText(name)
	m.breadcrumb = m.breadcrumb.SetUntitledName(name + ".md")
	return m
}

func (m Model) SetFilePath(path string) Model {
	m.filePath = path
	m.breadcrumb = m.breadcrumb.SetPath(path)
	stem := strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))
	m.title = m.title.SetText(stem)
	m.breadcrumb = m.breadcrumb.SetUntitledName(filepath.Base(path))
	return m
}

func (m Model) BreadcrumbView() string {
	return m.breadcrumb.View()
}
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

// SetHighlighter replaces the code highlighter adapter. Used for testing.
func (m Model) SetHighlighter(h CodeHighlighter) Model { m.highlighter = h; return m }

// SetDirtyForTest marks the editor dirty without modifying content. Test-only.
func (m Model) SetDirtyForTest() Model { m.dirty = true; return m }

func PreferredWidth() int { return 40 }

// isPrintableChar reports whether the rune is a printable ASCII character.
func isPrintableChar(r rune) bool {
	return r >= ' ' && r <= '~'
}

// firstRune returns the first rune and its byte size from s.
func firstRune(s string) (rune, int) {
	return utf8.DecodeRuneInString(s)
}
