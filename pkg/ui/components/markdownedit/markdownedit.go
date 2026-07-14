package markdownedit

import (
	"path/filepath"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/display"
	"rune/pkg/editor/keybind"
	"rune/pkg/imagekit"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/image"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
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
	placedRegions       map[string]placedRegion // iTerm2: last on-screen region per image, for erase-on-change

	publishedDims map[string]display.ImageDims // last dims pushed via SetImageDims; gates re-publishing (afterMutation)

	docPath string        // the open document's path (golden source; set with content). "" = untitled
	root    string        // workspace root (launch CWD); static fallback base for resolution
	fs      vfs.FS        // filesystem for link/embed resolution + image reads; always non-nil (§1.4.9) — New defaults it to vfs.Disk{}, SetFS overrides it
	styles  styles.Styles // cached for cell rendering
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

// New creates a new markdownedit Model. It does not set a SyncFunc: markdown
// parsing/concealment already runs inside textedit.PlainSync (the default,
// via display.SyntaxMap.Sync — §12); markdownedit's own contribution is
// render-time styling via CellBuilderFunc/ImageRowFunc (spanToCellsStyled),
// passed to RenderView, not through the SyncFunc seam. fs defaults to
// vfs.Disk{} so link/embed resolution + image reads see real disk until
// SetFS injects the workspace's own filesystem (§1.4.9); production, which
// never re-injects a different one, is byte-identical to direct os calls.
func New(keys keymap.Bindings, st styles.Styles, caps terminal.TermCaps, opts ...Option) Model {
	base := textedit.New(keys, st, opts...)
	return Model{
		Model:         base,
		highlighter:   ChromaHighlighter(),
		termCaps:      caps,
		imageConfig:   ImageConfig{AssetsDir: "assets"},
		images:        map[string]image.Model{},
		placedRegions: map[string]placedRegion{},
		idAlloc:       newImageIDAllocator(),
		cellSize:      imagekit.DefaultCellSize(),
		fs:            vfs.Disk{},
		styles:        st,
	}
}

func (m Model) Init() tea.Cmd { return m.Model.Init() }

// applyChecked runs a §1.3 fallible embedded-textedit mutation and, on
// success, funnels the result through afterMutation(true) — the shared shape
// ApplyInverse/Reapply/ReplaceRange all need. On error the buffer is left
// unchanged (op's own guarantee) and the caller must surface the failure.
func (m Model) applyChecked(op func(textedit.Model) (textedit.Model, error)) (Model, tea.Cmd, error) {
	tm, err := op(m.Model)
	if err != nil {
		return m, nil, err
	}
	m.Model = tm
	rm, cmd := m.afterMutation(true)
	return rm, cmd, nil
}

// ApplyInverse shadows textedit.Model.ApplyInverse, also running afterMutation.
// A non-nil error means the inverse edits did not fit the buffer (§1.3): the buffer
// is left unchanged and the caller surfaces the failure instead of advancing undo.
func (m Model) ApplyInverse(edits []buffer.AppliedEdit) (Model, tea.Cmd, error) {
	return m.applyChecked(func(tm textedit.Model) (textedit.Model, error) {
		return tm.ApplyInverse(edits)
	})
}

// Reapply shadows textedit.Model.Reapply, also running afterMutation.
// A non-nil error means a redo edit was out of bounds (§1.3); the buffer is left
// unchanged so the caller can keep the journal position coherent with it.
func (m Model) Reapply(edits []buffer.AppliedEdit) (Model, tea.Cmd, error) {
	return m.applyChecked(func(tm textedit.Model) (textedit.Model, error) {
		return tm.Reapply(edits)
	})
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
		// Text paste: ordinary delegation — it bumps rev, so reconcile's
		// contentChanged branch already does the right thing.
		return m.delegateToModel(msg)

	case ImageSavedMsg:
		return m.handleImageSaved(msg.RelativePath, time.Now())

	case ImageErrorMsg:
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

	case tea.KeyPressMsg:
		// Enter and Ctrl+Enter are aliases that follow the link under a lone
		// caret instead of inserting a newline (a newline inside a link would
		// break it). Gated to a single caret with no selection so a multi-cursor
		// or selection edit is never hijacked. This pre-empts textedit's newline
		// (textedit.go updateKeys); every other key falls through to it.
		if msg.Code == tea.KeyEnter && (msg.Mod == 0 || msg.Mod == tea.ModCtrl) &&
			m.Model.SingleCaretNoSelection() {
			if la, ok := m.linkAt(m.Model.CursorOffset()); ok {
				return m, func() tea.Msg { return la }
			}
		}
		return m.delegateToModel(msg)

	default:
		// All other messages go to textedit, then afterContentChange.
		return m.delegateToModel(msg)
	}
}

// delegateToModel forwards a message to the embedded textedit and reconciles
// image state afterward via reconcile.
func (m Model) delegateToModel(msg tea.Msg) (Model, tea.Cmd) {
	prevRev := m.Model.Revision()
	var cmd tea.Cmd
	m.Model, cmd = m.Model.Update(msg)
	m, rCmd := m.reconcile(prevRev)
	return m, tea.Batch(cmd, rCmd)
}

// reconcile is the shared funnel: it diffs prevRev (captured before the
// embedded textedit's Update ran) against the CURRENT textedit.Model.Revision()
// — the sanctioned change signal (D13, see textedit.Model.Revision's doc
// comment) — and hands the verdict to afterMutation, markdownedit's single
// post-mutation funnel. Every entry point that mutates the embedded
// textedit.Model (today: delegateToModel; any future one) funnels through
// this one comparison so no caller can drift from it.
func (m Model) reconcile(prevRev uint64) (Model, tea.Cmd) {
	return m.afterMutation(m.Model.Revision() != prevRev)
}

// SetRect sets position and size. Overrides textedit.SetRect to also resize
// images and run afterMutation(false) — folding in what
// RefreshImagesAfterLayoutChange used to do separately (retransmit + the
// dims/collapse/view-state funnel), since SetRect never touches the buffer.
// Edge E2: signature is (Model, tea.Cmd); the workspace's one caller
// (recalcLayout) must not drop the returned Cmd.
func (m Model) SetRect(r textedit.Rect) (Model, tea.Cmd) {
	// The workspace re-runs recalcLayout (→ SetRect) on every keypress, almost
	// always with unchanged dimensions. Re-publishing image dims and following
	// the cursor on those no-op passes would re-pin the viewport to the cursor
	// and clobber an intentional scroll (e.g. PageDown, or arrow-scroll in a
	// read-only doc whose hidden cursor sits at the top). Only do that work when
	// the rect actually changed — mirroring textedit.SetRect's own guard.
	changed := r.W != m.Model.Width() || r.H != m.Model.Height()
	m.Model = m.Model.SetRect(r)
	if !changed {
		return m, nil
	}
	g := m.Model.Geom()
	m = m.resizeImages(g.ImageMaxCols(), g.ContentHeight)

	var cmds []tea.Cmd
	if cmd := m.retransmitImagesCmd(); cmd != nil {
		cmds = append(cmds, cmd)
	}
	var cmd tea.Cmd
	m, cmd = m.afterMutation(false)
	if cmd != nil {
		cmds = append(cmds, cmd)
	}
	return m, tea.Batch(cmds...)
}

// SetContent replaces buffer content and runs afterMutation(true) — discovery,
// dims republish, collapse detect, and view-state re-arm for the freshly-set
// document. Edge E1: signature is (Model, tea.Cmd); this Cmd IS the image
// discovery for the new content — every non-test caller must unchain and
// append it (never drop it), or embeds stay unspawned until the next mutation.
func (m Model) SetContent(content string) (Model, tea.Cmd) {
	m.Model = m.Model.SetContent(content)
	return m.afterMutation(true)
}

// SetDocPath pins the open document's path — the golden source for resolving its
// relative links and image embeds (docDir = filepath.Dir(docPath); "" = untitled).
// The workspace projects this from its single source of truth (docView) at one
// authority point (finalize), so it tracks EVERY transition — load, untitled,
// help, bind-new, rename — and can never desync from the displayed document.
func (m Model) SetDocPath(path string) Model {
	m.docPath = path
	return m
}

// SetRoot sets the workspace root (launch CWD) — the static fallback base for
// resolving relative refs. Set once; it never changes during a run.
func (m Model) SetRoot(root string) Model {
	m.root = root
	return m
}

// DocPath returns the pinned document path (the resolution base source). Exposed
// for tests asserting the workspace projects it from its single source of truth.
func (m Model) DocPath() string { return m.docPath }

// SetFS injects the filesystem used for link/embed resolution and image-byte
// reads. The workspace propagates its own vfs.FS here so the editor's existence
// checks see the SAME files it loads from (§1.4.9).
func (m Model) SetFS(fsys vfs.FS) Model {
	m.fs = fsys
	return m
}

// docDir is the directory the open document lives in — the base for resolving its
// relative links and image embeds, derived from the golden path. "" for an untitled
// doc (resolution then uses only the workspace root).
func (m Model) docDir() string {
	if m.docPath == "" {
		return ""
	}
	return filepath.Dir(m.docPath)
}

// ReplaceRange replaces the range [start, end) with text and runs
// afterMutation(true). Propagates textedit.ReplaceRange's §1.3 bounds error
// unchanged — the buffer is left untouched on error, so the caller MUST
// surface it (e.g. via the workspace's errorCmd) rather than drop it.
func (m Model) ReplaceRange(start, end int, text string) (Model, tea.Cmd, error) {
	return m.applyChecked(func(tm textedit.Model) (textedit.Model, error) {
		return tm.ReplaceRange(start, end, text)
	})
}

// ReplaceAll replaces the entire buffer content in one journaled edit. Used by
// the conflict [D]iscard path (loads theirs) and by mergemode.Enter/Abort (loads
// the marker buffer / reverts to pre-merge ours) — merge semantics live entirely
// in the mergemode package (§10); this is a plain whole-buffer replace.
func (m Model) ReplaceAll(content string) (Model, tea.Cmd, error) {
	return m.ReplaceRange(0, len(m.Model.Content()), content)
}
