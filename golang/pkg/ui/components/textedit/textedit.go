package textedit

import (
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/coords"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
	"rune/pkg/editor/keybind"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// SyncFunc produces a SyntaxMap and SyntaxSnapshot from the buffer.
// textedit's syncDisplay handles wrapMap.Sync, BuildSnapshot, and ExpandTableRows.
type SyncFunc func(buf buffer.Buffer, sm display.SyntaxMap, cursors cursor.CursorSet, focused bool, width int) (display.SyntaxMap, display.SyntaxSnapshot)

// PlainSync is the default SyncFunc. It does not skip markdown parsing —
// display.SyntaxMap.Sync/SyncNoReveal parse and conceal markdown syntax for
// every textedit, PlainSync included (§12); the difference between plain
// textedit and markdownedit is entirely at render time, via the separate
// CellBuilderFunc/ImageRowFunc seam (markdownedit.spanToCellsStyled is what
// markdown syntax highlighting actually is — §12). PlainSync itself only
// picks Sync vs SyncNoReveal by focus.
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

// Model is the text-editing component (no markdown rendering). Its immediate
// helpers are split across sibling files by concern: mouse.go (low-level
// mouse-to-buffer helpers), viewport.go (ViewportState + scroll), edit_
// primitives.go (ReplaceRange/ApplyInverse/Reapply), render.go
// (syncDisplay + the renderCells/View pipeline).
type Model struct {
	buf                   buffer.Buffer
	cursors               cursor.CursorSet
	pendingEdits          []buffer.AppliedEdit
	lastEdits             []buffer.Edit // pre-edit-coordinate, CursorID-tagged edits from the message just processed — see FuzzLastEdits
	softWrap              bool
	indent                IndentConfig
	syntaxMap             display.SyntaxMap
	wrapMap               display.WrapMap
	snapshot              display.DisplaySnapshot
	imageDims             map[string]display.ImageDims // per-image cell footprints; drives image-row expansion in syncDisplay
	syntaxSnap            display.SyntaxSnapshot
	wrapSnap              display.WrapSnapshot
	resolver              keybind.Resolver
	registry              command.Registry
	viewport              ViewportState
	styles                styles.Styles
	width                 int
	height                int
	offsetX               int
	offsetY               int
	focused               bool
	searchMatches         []SelInterval
	searchActive          ActiveMatch // Valid=false when no match is active (§1.7)
	searchQuery           string      // last query set via SetSearchQuery
	searchCaseInsensitive bool
	searchRev             uint64 // m.rev when searchMatches was last computed
	syncFunc              SyncFunc
	sanitizeFunc          SanitizeFunc
	singleLine            bool
	readOnly              bool
	headerHeight          int
	rev                   uint64       // monotonic buffer-mutation counter (D13)
	bgIntervals           []BgInterval // caller-set background tints, applied in renderCells (SetBackgroundIntervals)
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

// New creates a new textedit Model. keys is accepted for construction-site
// symmetry with sibling components (chat/title/search/markdownedit all take
// the same keymap.Bindings) but is not itself retained on Model — textedit
// only ever consults it indirectly, through the resolver/registry built from
// it by the caller.
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
		styles:       st,
		syncFunc:     PlainSync,
		sanitizeFunc: func(s string) string { return s },
	}
	for _, opt := range opts {
		opt(&m)
	}
	return m
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
		m = m.ScrollToCursor()
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

// Revision returns a monotonic buffer-mutation counter (D13). This is the
// SANCTIONED content-changed signal: a caller that needs to know whether an
// Update call actually mutated the buffer diffs Revision() before/after
// (markdownedit.reconcile does exactly this) rather than being told via an
// async message or callback — Update is already synchronous and Model is a
// value, so a before/after diff is sufficient and simpler.
func (m Model) Revision() uint64 { return m.rev }

// CursorOffsets returns cursor byte offsets for overlay rendering.
func (m Model) CursorOffsets() map[int]bool {
	offs := make(map[int]bool)
	for _, c := range m.cursors.All() {
		offs[c.Position] = true
	}
	return offs
}

// Selections returns selection intervals for overlay rendering and external
// consumers (e.g. image paste). For reversed selections the End is advanced
// past the anchor character so it is included in the interval.
func (m Model) Selections() []SelInterval {
	var sels []SelInterval
	for _, c := range m.cursors.All() {
		if c.HasSelection() {
			end := selectionEndInclusive(c, m.buf)
			sels = append(sels, SelInterval{Start: c.SelectionStart(), End: end})
		}
	}
	return sels
}

// Focused returns whether this component is focused.
func (m Model) Focused() bool { return m.focused }

// ReadOnly returns whether the editor is in read-only mode.
func (m Model) ReadOnly() bool { return m.readOnly }

// SetReadOnly sets read-only mode.
func (m Model) SetReadOnly(ro bool) Model {
	m.readOnly = ro
	return m
}

// SetBackgroundIntervals sets the byte-range background tints applied in
// renderCells (before sliceCells). A generic, merge-agnostic overlay — see
// BgInterval / ApplyBackgroundIntervals in cell.go.
func (m Model) SetBackgroundIntervals(ivs []BgInterval) Model {
	m.bgIntervals = ivs
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
	m.lastEdits = nil
	m.viewport.TopRow = 0
	m.viewport.ScrollCol = 0
	m.rev++ // D13: a full content swap is a buffer mutation — see recomputeIfStale.
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

// SetCursors restores cursor state and scrolls to the primary cursor. It re-runs
// syncDisplay because the markdown reveal layer is a function of cursor position
// (a span reveals its raw source when a caret is inside it): undo/redo restore the
// caret here AFTER ApplyInverse/Reapply already laid out the display against the
// pre-restore cursors, so without this the caret can land on hidden markup (a
// heading's "# ", a link's delimiters) that has no rendered cell — leaving it
// invisible until the next keystroke forces a re-render (R1).
func (m Model) SetCursors(cs []cursor.Cursor) Model {
	if len(cs) > 0 {
		m.cursors = cursor.NewCursorSetFrom(cs)
		m = m.syncDisplay()
		m = m.ScrollToCursor()
	}
	return m
}

// ---- Exported seams for markdownedit composition (D1, D2, D3, D5, D11) ----

// SetImageDims sets the per-image cell footprints that drive standalone-image
// row expansion, then rebuilds the display so the snapshot reflects them
// immediately. markdownedit pushes this whenever image state changes (decode,
// transmit, resize). A nil/empty map leaves every image line collapsed to one
// row. Rebuilding from syncDisplay (rather than re-expanding the live snapshot)
// keeps expansion idempotent — ExpandImageRows is only ever applied to a freshly
// built, unexpanded snapshot.
func (m Model) SetImageDims(dims map[string]display.ImageDims) Model {
	m.imageDims = dims
	return m.syncDisplay()
}

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

// CursorOffset returns the primary cursor's byte offset.
func (m Model) CursorOffset() int {
	return m.cursors.Primary().Position
}

// OffsetToLineCol converts a buffer byte offset to a buffer line/column point.
// Exposed so markdownedit can locate the syntax span (e.g. a link) under the
// caret, which is keyed by buffer line.
func (m Model) OffsetToLineCol(offset int) coords.BufferPoint {
	return m.buf.OffsetToLineCol(offset)
}

// SingleCaretNoSelection reports whether there is exactly one cursor with no
// active selection. Callers gate caret-position-sensitive actions on this so a
// multi-cursor or selection edit is never hijacked.
func (m Model) SingleCaretNoSelection() bool {
	return m.cursors.Len() == 1 && !m.cursors.Primary().HasSelection()
}

// Width returns the allocated width.
func (m Model) Width() int { return m.width }

// ---- applyOperation (generic, no image/save handling) ----

func (m Model) applyOperation(result command.Result) Model {
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
		// A no-op result's Cursors is EXPECTED to carry the pass-through,
		// unchanged set (clipboardCopy/clipboardPaste do this explicitly:
		// "Cursors: ctx.Cursors", since they still launch a Cmd). But many
		// "nothing to do here" early returns across commands_edit.go/
		// commands_multi.go/commands_edit_lines_*.go build a bare
		// command.Result{Operation: command.Operation{Kind: OperationNone}}
		// with no Cursors field at all — its zero value is an EMPTY
		// CursorSet, not "unchanged". Assigning that unconditionally wiped
		// the editor's real cursor out of existence on the most ordinary
		// no-op there is (Backspace at buffer position 0) — found via
		// FuzzHumanSession's unicodeTyping cluster (WP6 session): once
		// cursors is empty, EVERY subsequent command also takes the
		// len(cursors)==0 fast path, permanently locking the buffer with no
		// caret. An editor with focus always has >= 1 cursor (M1's own
		// invariant); a no-op can never legitimately reduce that to zero,
		// so an empty result here is unconditionally a bug in the
		// producing command, not a real "clear all cursors" intent —
		// preserve the model's current cursors instead of adopting it.
		if result.Operation.Cursors.Len() > 0 {
			m.cursors = result.Operation.Cursors
		}
		return m
	}

	if len(result.Operation.Edits) > 0 {
		// A failed ApplyEdits (stale/out-of-bounds positions) leaves buf/
		// pendingEdits/rev/lastEdits untouched — D13 tightening (T3): rev is
		// the ONLY sanctioned content-changed signal, and nothing actually
		// changed. Cursors are still adopted below either way, matching
		// prior behavior; markdownedit's reconcile funnel then sees rev
		// unchanged and correctly takes its cursor-move branch, not a
		// content-change one.
		if newBuf, applied, err := m.buf.ApplyEdits(result.Operation.Edits); err == nil {
			m = m.commitEdits(newBuf, applied, true, result.Operation.Edits)
		}
	}

	m.cursors = result.Operation.Cursors
	return m
}

// commitEdits is the single buf-swap / pendingEdits / lastEdits / rev++ site
// (D13): every content mutation funnels through here so "did the buffer
// change" has exactly one answer. journal=false for undo/redo (ApplyInverse/
// Reapply) — the journal already recorded the edit being undone/redone, so
// re-appending it to pendingEdits would double-journal it. provenance is the
// pre-edit-coordinate, CursorID-tagged edits behind this commit (nil for
// programmatic/undo/redo commits, which aren't attributable to a single
// cursor) — see FuzzLastEdits.
func (m Model) commitEdits(newBuf buffer.Buffer, applied []buffer.AppliedEdit, journal bool, provenance []buffer.Edit) Model {
	m.buf = newBuf
	if journal {
		m.pendingEdits = append(m.pendingEdits, applied...)
	}
	m.lastEdits = provenance
	m.rev++
	return m
}

// applyResult is the per-keypress dispatch epilogue: apply the operation,
// resync display, then follow the cursor into view — unless the operation
// IS an intentional scroll, in which case following the cursor would cancel
// it (critical for read-only docs whose hidden cursor sits at the top). This
// one function replaces what used to be five near-identical
// applyOperation+syncDisplay+ScrollToCursor trios (update.go's Enter/Escape
// fast paths and resolver dispatch, commands_clipboard.go's paste).
func (m Model) applyResult(res command.Result) Model {
	m = m.applyOperation(res)
	m = m.syncDisplay()
	if res.Operation.Kind != command.OperationScroll {
		m = m.ScrollToCursor()
	}
	return m
}

// basicCtx builds the minimal CommandContext used by the hardcoded
// Enter/Escape fast paths (edit.newline, multicursor.escape) — neither
// command needs the resolver dispatch's navigation/viewport capabilities.
func (m Model) basicCtx() command.CommandContext {
	return command.CommandContext{Buffer: m.buf, Cursors: m.cursors}
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
