package textedit

import (
	"strings"
	"unicode/utf8"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"charm.land/bubbles/v2/key"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/coords"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
	"rune/pkg/editor/keybind"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

// SyncFunc produces a SyntaxMap and SyntaxSnapshot from the buffer.
// textedit's syncDisplay handles wrapMap.Sync, BuildSnapshot, and ExpandTableRows.
type SyncFunc func(buf buffer.Buffer, sm display.SyntaxMap, cursors cursor.CursorSet, focused bool, width int) (display.SyntaxMap, display.SyntaxSnapshot)

// PlainSync is the default SyncFunc: no markdown rendering, just text.
func PlainSync(buf buffer.Buffer, sm display.SyntaxMap, cursors cursor.CursorSet, focused bool, width int) (display.SyntaxMap, display.SyntaxSnapshot) {
	if sm == (display.SyntaxMap{}) {
		sm = display.NewSyntaxMap()
	}
	sm = sm.SetWidth(max(width, 0))
	if focused {
		return sm.Sync(buf, cursors)
	}
	return sm.SyncNoReveal(buf, cursors)
}

// SanitizeFunc sanitizes text before insertion.
type SanitizeFunc func(s string) string

// Model is the text-editing component (no markdown rendering).
type Model struct {
	buf              buffer.Buffer
	cursors          cursor.CursorSet
	pendingEdits     []buffer.AppliedEdit
	softWrap         bool
	indent           IndentConfig
	syntaxMap        display.SyntaxMap
	wrapMap          display.WrapMap
	snapshot         display.DisplaySnapshot
	syntaxSnap       display.SyntaxSnapshot
	wrapSnap         display.WrapSnapshot
	resolver         keybind.Resolver
	registry         command.Registry
	viewport         ViewportState
	keys             keymap.Bindings
	styles           styles.Styles
	width            int
	height           int
	offsetX          int
	offsetY          int
	focused          bool
	findOverlay      FindOverlay
	syncFunc         SyncFunc
	sanitizeFunc     SanitizeFunc
	singleLine       bool
	readOnly         bool
	headerHeight     int
	rev              uint64 // monotonic buffer-mutation counter (D13)
}

type ViewportState struct {
	TopRow    int
	ScrollCol int
}

type IndentConfig struct {
	UseTabs bool
	TabSize int
}

// Option configures a Model during construction.
type Option func(*Model)

// WithSyncFunc sets the display-sync function.
func WithSyncFunc(f SyncFunc) Option {
	return func(m *Model) {
		m.syncFunc = f
	}
}

// WithSanitizeFunc sets the text-sanitization function.
func WithSanitizeFunc(f SanitizeFunc) Option {
	return func(m *Model) {
		m.sanitizeFunc = f
	}
}

// WithSingleLine disables newline insertion.
func WithSingleLine() Option {
	return func(m *Model) {
		m.singleLine = true
	}
}

// WithReadOnly makes the editor read-only (no mutations, no caret).
func WithReadOnly() Option {
	return func(m *Model) {
		m.readOnly = true
	}
}

// WithPaddingTop sets the header height (rows above content).
func WithPaddingTop(n int) Option {
	return func(m *Model) {
		m.headerHeight = n
	}
}

// WithRegistry sets the command registry. Must be pre-built from textedit.RegisterCommands.
func WithRegistry(reg command.Registry) Option {
	return func(m *Model) { m.registry = reg }
}

// WithResolver sets the keybind resolver. Must include textedit command bindings.
func WithResolver(resolver keybind.Resolver) Option {
	return func(m *Model) { m.resolver = resolver }
}

// New creates a new textedit Model.
func New(keys keymap.Bindings, st styles.Styles, opts ...Option) Model {
	resolver, _ := keybind.NewResolver(nil)
	m := Model{
		buf:          buffer.New(""),
		cursors:      cursor.NewCursorSet(0),
		softWrap:     true,
		indent:       IndentConfig{UseTabs: false, TabSize: 4},
		resolver:     resolver,
		registry:     command.NewBuilder().Build(),
		viewport:     ViewportState{},
		keys:         keys,
		styles:       st,
		syncFunc:     PlainSync,
		sanitizeFunc: func(s string) string { return s },
	}
	for _, opt := range opts {
		opt(&m)
	}
	return m
}

func (m Model) Init() tea.Cmd { return nil }

// Update handles messages and returns accumulated commands.
func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	switch msg := msg.(type) {
	case ClipboardContentMsg:
		if len(msg.ImageData) > 0 {
			// Image paste is a no-op in plain textedit
			return m, nil
		}
		var cmd tea.Cmd
		m, cmd = m.handlePasteContent(msg.Text)
		return m, cmd

	case tea.ClipboardMsg:
		if !m.focused {
			return m, nil
		}
		var cmd tea.Cmd
		m, cmd = m.handlePasteContent(msg.Content)
		return m, cmd

	case tea.PasteMsg:
		if !m.focused {
			return m, nil
		}
		var cmd tea.Cmd
		m, cmd = m.handlePasteContent(msg.Content)
		return m, cmd

	case tea.WindowSizeMsg:
		// Window size is handled by the parent via SetRect; children do NOT
		// handle tea.WindowSizeMsg directly (per CLAUDE.md component contracts).
		return m, nil

	case tea.KeyPressMsg:
		return m.updateKeys(msg, &cmds)
	}
	return m, tea.Batch(cmds...)
}

func (m Model) updateKeys(msg tea.KeyPressMsg, cmds *[]tea.Cmd) (Model, tea.Cmd) {
	if !m.focused {
		return m, nil
	}

	// Find overlay open commands (Cmd+F, Cmd+H) work regardless of overlay state
	if key.Matches(msg, m.keys.FindOpen) {
		m.findOverlay = m.findOverlay.open(false)
		return m, nil
	}
	if key.Matches(msg, m.keys.FindReplaceOpen) {
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

	// PrimaryAction: Enter key routes directly to edit.newline (no resolver binding)
	if msg.Code == tea.KeyEnter && msg.Mod == 0 {
		if m.singleLine {
			// No-op in single-line mode
			return m, nil
		}
		ctx := command.CommandContext{
			Buffer:  m.buf,
			Cursors: m.cursors,
		}
		res := m.registry.Execute("edit.newline", ctx)
		if res.Err == nil {
			m = m.applyOperation(res, "edit.newline")
			m = m.syncDisplay()
			m = m.ScrollToCursor()
			return m, tea.Batch(*cmds...)
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
			m = m.applyOperation(res, "multicursor.escape")
			m = m.syncDisplay()
			m = m.ScrollToCursor()
			return m, tea.Batch(*cmds...)
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
		ReadOnly:       m.readOnly,
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
			FilePath:       "", // textedit doesn't own file path
			NewRequestID:   func() string { return "" },
			HashContent:    func(string) string { return "" },
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
		if res.Cmd != nil {
			*cmds = append(*cmds, res.Cmd)
		}
		m = m.applyOperation(res, resResult.Command)
		m = m.syncDisplay()
		m = m.ScrollToCursor()
	case keybind.ResultMoreChordsNeeded:
		// Chord incomplete — wait for next key
	case keybind.ResultNoMatch:
		text := msg.Text
		if text == "" && msg.Mod == 0 {
			code := msg.BaseCode
			if code == 0 {
				code = msg.Code
			}
			if isPrintableChar(code) {
				text = string(code)
			}
		}
		if text != "" {
			// Guard: read-only blocks printable char insertion
			if m.readOnly {
				return m, tea.Batch(*cmds...)
			}
			res := m.registry.Execute("edit.insert-character", command.CommandContext{
				Buffer:  m.buf,
				Cursors: m.cursors,
				Args:    map[string]any{"char": text},
			})
			if res.Err == nil && res.Operation.Kind != command.OperationNone {
				m = m.applyOperation(res, "edit.insert-character")
				m = m.syncDisplay()
				m = m.ScrollToCursor()
			}
		}
	}

	return m, tea.Batch(*cmds...)
}

func (m Model) contentHeight() int {
	h := m.height - m.headerHeight
	if h < 1 {
		return 1
	}
	return h
}

// imageMaxCols returns the maximum column width for rendered images.
func (m Model) imageMaxCols() int {
	w := m.width - 2
	if w < 1 {
		return 1
	}
	return w
}

// SetRect sets position and size atomically (D8).
func (m Model) SetRect(r Rect) Model {
	changed := r.W != m.width || r.H != m.height
	m.width = r.W
	m.height = r.H
	m.offsetX = r.X
	m.offsetY = r.Y
	if changed {
		m = m.syncDisplay()
	}
	return m
}

func (m Model) Height() int { return m.height }

func (m Model) SetFocused(f bool) Model {
	changed := m.focused != f
	m.focused = f
	// Resync display when focus changes because reveal decisions depend on it.
	if changed && m.buf.Content() != "" {
		m = m.syncDisplay()
	}
	return m
}

func (m Model) Content() string { return m.buf.Content() }

// Revision returns a monotonic buffer-mutation counter (D13).
func (m Model) Revision() uint64 { return m.rev }

// CursorOffsets returns cursor byte offsets for overlay rendering.
func (m Model) CursorOffsets() map[int]bool {
	offs := make(map[int]bool)
	for _, c := range m.cursors.All() {
		offs[c.Position] = true
	}
	return offs
}

// Selections returns selection intervals for overlay rendering.
func (m Model) Selections() []SelInterval {
	var sels []SelInterval
	for _, c := range m.cursors.All() {
		if c.HasSelection() {
			sels = append(sels, SelInterval{c.SelectionStart(), c.SelectionEnd()})
		}
	}
	return sels
}

// Focused returns whether this component is focused.
func (m Model) Focused() bool { return m.focused }

// WantsModalInput returns whether a modal overlay (find) is active.
func (m Model) WantsModalInput() bool { return m.findOverlay.visible }

// ReadOnly returns whether the editor is in read-only mode.
func (m Model) ReadOnly() bool { return m.readOnly }

// SetReadOnly sets read-only mode.
func (m Model) SetReadOnly(ro bool) Model {
	m.readOnly = ro
	return m
}

// SetContent replaces buffer content, resets cursors, syncs display, and
// clamps TopRow to [0, maxTop]. No baked scroll policy — caller chooses (D5).
func (m Model) SetContent(content string) Model {
	b, err := buffer.FromBytes([]byte(content))
	if err != nil {
		return m
	}
	m.buf = b
	m.cursors = cursor.NewCursorSet(0)
	m.pendingEdits = nil
	m.viewport.TopRow = 0
	m.viewport.ScrollCol = 0
	m = m.syncDisplay()
	m = m.clampScroll()
	return m
}

// DrainEdits returns and clears pending edits accumulated since last drain.
func (m Model) DrainEdits() (Model, []buffer.AppliedEdit) {
	edits := m.pendingEdits
	m.pendingEdits = nil
	return m, edits
}

// Cursors returns the current cursor/selection state.
func (m Model) Cursors() []cursor.Cursor {
	return m.cursors.All()
}

// SetCursors restores cursor state and scrolls to the primary cursor.
func (m Model) SetCursors(cs []cursor.Cursor) Model {
	if len(cs) > 0 {
		m.cursors = cursor.NewCursorSetFrom(cs)
		m = m.ScrollToCursor()
	}
	return m
}

// ApplyInverse applies the inverse of the given edits (undo).
// Does NOT accumulate into pendingEdits.
func (m Model) ApplyInverse(edits []buffer.AppliedEdit) Model {
	inverse := make([]buffer.Edit, len(edits))
	for i, ae := range edits {
		inverse[i] = buffer.Edit{
			Start:  ae.Start,
			End:    ae.Start + len(ae.Insert),
			Insert: ae.Deleted,
		}
	}
	inverse = buffer.CloneAndSortEditsDescending(inverse)
	newBuf, _, err := m.buf.ApplyEdits(inverse)
	if err == nil {
		m.buf = newBuf
	}
	m.rev++
	m = m.syncDisplay()
	return m
}

// Reapply applies the given edits forward (redo).
// Does NOT accumulate into pendingEdits.
func (m Model) Reapply(edits []buffer.AppliedEdit) Model {
	fwdEdits := make([]buffer.Edit, len(edits))
	cumulativeShift := 0
	for i := len(edits) - 1; i >= 0; i-- {
		ae := edits[i]
		originalStart := ae.Start - cumulativeShift
		fwdEdits[i] = buffer.Edit{
			Start:  originalStart,
			End:    originalStart + len(ae.Deleted),
			Insert: ae.Insert,
		}
		cumulativeShift += len(ae.Insert) - len(ae.Deleted)
	}
	newBuf, _, err := m.buf.ApplyEdits(fwdEdits)
	if err == nil {
		m.buf = newBuf
	}
	m.rev++
	m = m.syncDisplay()
	return m
}

// clampScroll clamps viewport.TopRow to [0, maxTop].
func (m Model) clampScroll() Model {
	maxTop := m.snapshot.TotalRows - m.contentHeight()
	if maxTop < 0 {
		maxTop = 0
	}
	if m.viewport.TopRow < 0 {
		m.viewport.TopRow = 0
	}
	if m.viewport.TopRow > maxTop {
		m.viewport.TopRow = maxTop
	}
	return m
}

// AtBottom reports whether the viewport is at the bottom of the content.
func (m Model) AtBottom() bool {
	return m.viewport.TopRow >= m.snapshot.TotalRows-m.contentHeight()
}

// GotoBottom scrolls to the bottom of the content.
func (m Model) GotoBottom() Model {
	m.viewport.TopRow = max(0, m.snapshot.TotalRows-m.contentHeight())
	return m
}

// ScrollOffset returns the current TopRow.
func (m Model) ScrollOffset() int { return m.viewport.TopRow }

// SetScrollOffset sets TopRow, clamped to [0, maxTop].
func (m Model) SetScrollOffset(offset int) Model {
	maxTop := m.snapshot.TotalRows - m.contentHeight()
	if maxTop < 0 {
		maxTop = 0
	}
	if offset < 0 {
		offset = 0
	}
	if offset > maxTop {
		offset = maxTop
	}
	m.viewport.TopRow = offset
	return m
}

// NaturalContentHeight returns the visual height of content at the given width.
func (m Model) NaturalContentHeight(width int) int {
	sm := display.NewSyntaxMap()
	wm := display.NewWrapMap(0)
	sm = sm.SetWidth(width)
	sm, snapSnap := sm.Sync(m.buf, cursor.NewCursorSet(0))
	wm = wm.SetWidth(width)
	snap := display.BuildSnapshot(wm.Sync(snapSnap))
	snap = display.ExpandTableRows(snap)
	return snap.TotalRows
}

// ---- Exported seams for markdownedit composition (D1, D2, D3, D5, D11) ----

// Snapshot returns the current display snapshot.
func (m Model) Snapshot() display.DisplaySnapshot { return m.snapshot }

// SetSnapshot replaces the current display snapshot.
func (m Model) SetSnapshot(snap display.DisplaySnapshot) Model {
	m.snapshot = snap
	return m
}

// ScrollToCursor scrolls the viewport to make the primary cursor visible.
func (m Model) ScrollToCursor() Model {
	if len(m.cursors.All()) == 0 {
		return m
	}
	primary := m.cursors.Primary()
	bp := m.buf.OffsetToLineCol(primary.Position)
	sp := m.syntaxSnap.BufferToSyntax(bp)
	wp := m.wrapSnap.SyntaxToWrap(sp)

	contentH := m.contentHeight()

	// Map wrap-row to display-row (accounting for image/table expansion)
	modelLine := sp.Line
	wrapOffsetWithinLine := wp.Row - m.wrapSnap.ModelLineToFirstRow(modelLine)
	if wrapOffsetWithinLine < 0 {
		wrapOffsetWithinLine = 0
	}
	cursorDisplayRow := m.snapshot.ModelLineToFirstRow(modelLine) + wrapOffsetWithinLine

	if cursorDisplayRow < m.viewport.TopRow {
		m.viewport.TopRow = cursorDisplayRow
	} else if cursorDisplayRow >= m.viewport.TopRow+contentH {
		m.viewport.TopRow = cursorDisplayRow - contentH + 1
	}

	// Horizontal scroll
	if !m.softWrap {
		if wp.Col < m.viewport.ScrollCol {
			m.viewport.ScrollCol = wp.Col
		} else if wp.Col >= m.viewport.ScrollCol+m.width {
			m.viewport.ScrollCol = wp.Col - m.width + 1
		}
	}

	return m
}

// ContentHeight returns the allocated content height.
func (m Model) ContentHeight() int { return m.contentHeight() }

// ---- Low-level mouse helpers (D3) ----

// DisplayToBuffer converts a display-point to a buffer point and offset.
func (m Model) DisplayToBuffer(dp coords.DisplayPoint) (coords.BufferPoint, int) {
	wrapRow := dp.Row + m.viewport.TopRow
	wrapCol := dp.Col + m.viewport.ScrollCol
	if wrapRow < 0 {
		wrapRow = 0
	}
	if wrapRow >= m.wrapSnap.TotalRows {
		wrapRow = m.wrapSnap.TotalRows - 1
	}
	if wrapRow < 0 {
		return coords.BufferPoint{Line: 0, Col: 0}, 0
	}
	wp := coords.WrapPoint{Row: wrapRow, Col: wrapCol}
	sp := m.wrapSnap.WrapToSyntax(wp)
	bp := m.syntaxSnap.SyntaxToBuffer(sp)
	return bp, m.buf.LineColToOffset(bp)
}

// MousePositionCursor moves the primary cursor to the given buffer offset.
func (m Model) MousePositionCursor(offset int) Model {
	primary := cursor.Cursor{
		Position: offset,
		Anchor:   offset,
		ID:       1,
	}
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	return m
}

// MouseExtendSelection extends the primary cursor's selection to the given offset.
func (m Model) MouseExtendSelection(offset int) Model {
	primary := m.cursors.Primary()
	primary.Position = offset
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	return m
}

// MouseSelectWord selects the word at the given offset.
func (m Model) MouseSelectWord(offset int) Model {
	start := wordLeftOffset(m.buf, offset)
	end := wordRightOffset(m.buf, offset)
	primary := cursor.Cursor{
		Position: end,
		Anchor:   start,
		ID:       1,
	}
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	return m
}

// MouseSelectLine selects the line containing the given offset.
func (m Model) MouseSelectLine(line int) Model {
	lineStart := m.buf.LineStart(line)
	var lineEnd int
	if line >= m.buf.LineCount()-1 {
		lineEnd = m.buf.Len()
	} else {
		lineEnd = m.buf.LineStart(line + 1)
	}
	primary := cursor.Cursor{
		Position: lineEnd,
		Anchor:   lineStart,
		ID:       1,
	}
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	return m
}

// MouseAddCursor adds a new cursor at the given offset.
func (m Model) MouseAddCursor(offset int) Model {
	m.cursors = m.cursors.Add(cursor.Cursor{Position: offset, Anchor: offset})
	return m
}

// SyntaxSnap returns the syntax snapshot.
func (m Model) SyntaxSnap() display.SyntaxSnapshot { return m.syntaxSnap }

// WrapSnap returns the wrap snapshot.
func (m Model) WrapSnap() display.WrapSnapshot { return m.wrapSnap }

// Viewport returns the viewport state.
func (m Model) Viewport() ViewportState { return m.viewport }

// CursorAtTop reports whether all cursors are on visual row 0 and viewport is at top (D11).
func (m Model) CursorAtTop() bool {
	if m.viewport.TopRow != 0 {
		return false
	}
	for _, c := range m.cursors.All() {
		bp := m.buf.OffsetToLineCol(c.Position)
		sp := m.syntaxSnap.BufferToSyntax(bp)
		wp := m.wrapSnap.SyntaxToWrap(sp)
		if wp.Row > 0 {
			return false
		}
	}
	return true
}

// ---- Generic programmatic-edit primitives (D15) ----

// ReplaceRange replaces the range [start, end) with text. This is the core
// primitive: insert = ReplaceRange(off,off,t), delete = ReplaceRange(s,e,"").
func (m Model) ReplaceRange(start, end int, text string) Model {
	if m.readOnly {
		return m
	}
	if start > end {
		start, end = end, start
	}
	edit := buffer.Edit{Start: start, End: end, Insert: text}
	sorted := buffer.CloneAndSortEditsDescending([]buffer.Edit{edit})
	newBuf, applied, err := m.buf.ApplyEdits(sorted)
	if err != nil {
		return m
	}
	m.buf = newBuf
	m.pendingEdits = append(m.pendingEdits, applied...)
	m.cursors = cursor.NewCursorSet(start + utf8.RuneCountInString(text))
	m.rev++
	m = m.syncDisplay()
	m = m.ScrollToCursor()
	return m
}

// AppendText appends text at the primary cursor position.
func (m Model) AppendText(text string) Model {
	if m.readOnly {
		return m
	}
	primary := m.cursors.Primary()
	offset := primary.Position
	return m.ReplaceRange(offset, offset, text)
}

// CursorOffset returns the primary cursor's byte offset.
func (m Model) CursorOffset() int {
	return m.cursors.Primary().Position
}

// SetDir sets the base directory for resolving relative image embeds.
// In textedit this is a no-op (images are markdownedit concern), but the
// method exists for API compatibility with markdownedit.
func (m Model) SetDir(dir string) Model {
	_ = dir // no-op in textedit
	return m
}

// Width returns the allocated width.
func (m Model) Width() int { return m.width }

// OffsetX returns the component's screen X position.
func (m Model) OffsetX() int { return m.offsetX }

// OffsetY returns the component's screen Y position.
func (m Model) OffsetY() int { return m.offsetY }

// ImageMaxCols returns the maximum column width for rendered images.
func (m Model) ImageMaxCols() int {
	w := m.width - 2
	if w < 1 {
		return 1
	}
	return w
}

// ---- applyOperation (generic, no image/save handling) ----

func (m Model) applyOperation(result command.Result, cmdName string) Model {
	_ = cmdName // retained for future use (e.g., logging, metrics)

	if result.Operation.Kind == command.OperationScroll {
		m.viewport.TopRow += result.Operation.ScrollDY
		m.viewport.ScrollCol += result.Operation.ScrollDX
		m = m.clampScroll()
		if m.viewport.ScrollCol < 0 {
			m.viewport.ScrollCol = 0
		}
		return m
	}

	if result.Operation.Kind == command.OperationNone {
		m.cursors = result.Operation.Cursors
		return m
	}

	if len(result.Operation.Edits) > 0 {
		newBuf, applied, err := m.buf.ApplyEdits(result.Operation.Edits)
		if err == nil {
			m.buf = newBuf
			m.pendingEdits = append(m.pendingEdits, applied...)
		}
	}

	m.cursors = result.Operation.Cursors
	if result.Operation.Kind != command.OperationMoveCursors {
		m.rev++
	}
	return m
}

func (m Model) syncDisplay() Model {
	if m.syncFunc == nil {
		return m
	}
	width := m.width
	if width < 0 {
		width = 0
	}
	if m.wrapMap == (display.WrapMap{}) {
		m.wrapMap = display.NewWrapMap(0)
	}
	m.syntaxMap, m.syntaxSnap = m.syncFunc(m.buf, m.syntaxMap, m.cursors, m.focused, width)
	m.wrapMap = m.wrapMap.SetWidth(width)
	m.wrapSnap = m.wrapMap.Sync(m.syntaxSnap)
	m.snapshot = display.BuildSnapshot(m.wrapSnap)
	m.snapshot = display.ExpandTableRows(m.snapshot)
	return m
}

// ---- View ----

// renderCells builds the 2D cell grid for the given content height.
// It is called by both View() and the gated FuzzCells() accessor so the
// fuzzer checks exactly what the terminal renders — no drift.
func (m Model) renderCells(contentHeight int) [][]Cell {
	lines := m.snapshot.Slice(m.viewport.TopRow, contentHeight)

	cursorOffsets := make(map[int]bool)
	var selections []SelInterval
	if m.focused && !m.readOnly {
		for _, c := range m.cursors.All() {
			cursorOffsets[c.Position] = true
			if c.HasSelection() {
				selections = append(selections, SelInterval{c.SelectionStart(), c.SelectionEnd()})
			}
		}
	}
	// In read-only mode, keep selections visible but hide cursor
	if m.readOnly {
		for _, c := range m.cursors.All() {
			if c.HasSelection() {
				selections = append(selections, SelInterval{c.SelectionStart(), c.SelectionEnd()})
			}
		}
	}

	result := make([][]Cell, len(lines))
	for i, l := range lines {
		// Convert all spans to cells
		var lineCells []Cell
		for _, sp := range l.Spans {
			spCells := SpanToCells(sp, lipgloss.NewStyle())
			lineCells = append(lineCells, spCells...)
		}

		// EOL cursor: append synthetic cell if cursor is at end-of-line
		if m.focused && !m.readOnly {
			lineEnd := 0
			if len(l.Spans) > 0 {
				last := l.Spans[len(l.Spans)-1]
				lineEnd = last.BufferEnd
			}
			for off := range cursorOffsets {
				if off == lineEnd {
					isLastVisible := i+1 >= len(lines) || lines[i+1].ModelLine != l.ModelLine
					if isLastVisible {
						lineCells = append(lineCells, Cell{
							Rune:      ' ',
							Width:     1,
							Style:     lipgloss.NewStyle(),
							BufOffset: lineEnd,
						})
					}
					break
				}
			}
		}

		// Horizontal scrolling at cell level
		lineCells = SliceCells(lineCells, m.viewport.ScrollCol, m.width)

		// Apply cursor and selection overlays
		if m.focused && (len(cursorOffsets) > 0 || len(selections) > 0) {
			ApplyOverlays(lineCells, cursorOffsets, selections)
		}

		result[i] = lineCells
	}
	return result
}

func (m Model) View() string {
	if m.width == 0 || m.height == 0 {
		return ""
	}

	contentHeight := m.contentHeight()
	cells := m.renderCells(contentHeight)

	cursorStyle := lipgloss.NewStyle().Reverse(true)
	selStyle := m.styles.Selection

	var renderedLines []string
	for _, lineCells := range cells {
		renderedLines = append(renderedLines, CellsToString(lineCells, selStyle, cursorStyle))
	}

	for len(renderedLines) < contentHeight {
		renderedLines = append(renderedLines, "~")
	}

	content := strings.Join(renderedLines, "\n")
	if !m.focused {
		content = lipgloss.NewStyle().Faint(true).Render(content)
	}

	return lipgloss.NewStyle().
		MaxWidth(m.width).
		MaxHeight(m.height).
		Width(m.width).
		Height(m.height).
		Render(content)
}

// ---- Rect type (D8) ----

// Rect holds position and size for offset-bearing components.
type Rect struct {
	X, Y, W, H int
}

// ---- Helpers ----

func isPrintableChar(r rune) bool {
	return r >= ' ' && r <= '~'
}

func firstRune(s string) (rune, int) {
	return utf8.DecodeRuneInString(s)
}
