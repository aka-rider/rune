package markdownedit

import (
	"fmt"
	"maps"
	"strings"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/editor/display"
	"rune/pkg/ui/components/image"
	"rune/pkg/vfs"
)

// placedRegion is the on-screen footprint of one iTerm2 image at its last
// placement: a contiguous vertical run of cells. Stored per image so that when
// the image moves (scroll), shrinks (resize), or disappears, the OLD footprint
// can be blanked before the new one is drawn — the iTerm2 overlay does not erase
// itself the way Kitty's cell-grid placeholders do.
type placedRegion struct {
	screenRow int // 1-based first visible row
	rows      int // number of visible rows
	col       int // 1-based start column
	width     int // cell width to blank on erase
}

func (m Model) emitImagePlacements() (Model, tea.Cmd) {
	if !m.imageInlineCapable() {
		return m, nil
	}
	seq, regions := m.buildInlineImagePlacements()
	if seq == m.lastPlacementSeq {
		return m, nil
	}
	m.lastPlacementSeq = seq
	m.placedRegions = regions
	m.pendingPlacementSeq = ""

	if seq == "" {
		return m, nil
	}

	m.pendingPlacementSeq = seq
	return m, func() tea.Msg { return placementTickMsg{} }
}

type placementTickMsg struct{}

func (m Model) handlePlacementTick() (Model, tea.Cmd) {
	if m.pendingPlacementSeq == "" {
		return m, nil
	}
	seq := m.pendingPlacementSeq
	m.pendingPlacementSeq = ""
	return m, tea.Raw(seq)
}

// buildInlineImagePlacements returns the cursor-positioned OSC placement sequence
// for every live iTerm2 image row currently in view, plus the new per-image
// regions. Any previously-placed region that moved or vanished is blanked first:
// all erases precede all placements in the returned string (so an erase never
// clobbers another image's freshly-drawn pixels), and the whole batch is wrapped
// once in DECSC/DECRC (save/restore cursor).
func (m Model) buildInlineImagePlacements() (string, map[string]placedRegion) {
	g := m.Model.Geom()
	vp := g.Viewport
	contentH := g.ContentHeight
	screenBase := g.OffsetY
	col := g.OffsetX + 2 // 1-based terminal column + 1 left margin

	var place strings.Builder
	regions := map[string]placedRegion{}

	for lineIdx, l := range g.Snap.Lines {
		if l.ImagePath == "" {
			continue
		}
		img, ok := m.images[l.ImagePath]
		if !ok || !img.IsLive() || len(img.ITerm2Slices()) == 0 {
			continue
		}

		displayRow := lineIdx - vp.TopRow
		if displayRow < 0 || displayRow >= contentH {
			continue
		}

		slices := img.ITerm2Slices()
		if l.ImageRowIndex < 0 || l.ImageRowIndex >= len(slices) {
			continue
		}

		screenRow := screenBase + displayRow + 1 // +1 for 1-based terminal rows
		fmt.Fprintf(&place, "\033[%d;%dH%s", screenRow, col, slices[l.ImageRowIndex])

		// Accumulate this image's visible footprint (rows are contiguous).
		if r, seen := regions[l.ImagePath]; seen {
			if screenRow < r.screenRow {
				r.screenRow = screenRow
			}
			r.rows++
			regions[l.ImagePath] = r
		} else {
			regions[l.ImagePath] = placedRegion{screenRow: screenRow, rows: 1, col: col, width: l.ImageCols}
		}
	}

	// Erase every prior region that is now gone or has a different footprint.
	var erase strings.Builder
	for path, old := range m.placedRegions {
		if newR, still := regions[path]; still && newR == old {
			continue
		}
		for i := 0; i < old.rows; i++ {
			fmt.Fprintf(&erase, "\033[%d;%dH%s", old.screenRow+i, old.col, strings.Repeat(" ", old.width))
		}
	}

	body := erase.String() + place.String()
	if body == "" {
		return "", regions
	}
	return "\0337" + body + "\0338", regions // DECSC … DECRC
}

// spawnImage constructs a fresh image.Model for path/absPath/id/mtime and
// tracks it, returning its Init() decode Cmd — the one New+store+Init triple
// both the fresh-spawn and the mtime-respawn branch of syncImageSet need.
func (m Model) spawnImage(path, absPath string, id uint32, mtime int64, maxCols, maxRows int) (Model, tea.Cmd) {
	newImg := image.New(path, absPath, id, mtime, m.termCaps, m.cellSize, maxCols, maxRows, m.fs)
	m.images[path] = newImg
	return m, newImg.Init()
}

// syncImageSet reconciles the tracked image-instance set against the current
// snapshot in one pass: standalone (spawn source — isStandaloneImageLine,
// Rendered-only, unchanged from the pre-M6 discoverNewImages) drives
// spawn/respawn; present (any image-role span, Rendered OR Revealed — reveal-
// stable, so a caret pass over an embed line can never despawn a live image)
// drives despawn of any tracked path no longer referenced anywhere in the
// document (deleted embed line, undo/redo, ReplaceAll). A despawn frees the
// path's allocator IDs and its Kitty terminal-side pixel memory (iTerm2 needs
// nothing — its footprint is erased by the placement pipeline, §3.1).
func (m Model) syncImageSet() (Model, tea.Cmd) {
	if !m.imageCapable() {
		return m, nil
	}
	g := m.Model.Geom()
	maxCols := g.ImageMaxCols()
	maxRows := g.ContentHeight

	var standalone []string
	seen := map[string]bool{}
	for _, l := range g.Snap.Lines {
		path, ok := display.StandaloneImagePath(l)
		if !ok || seen[path] {
			continue
		}
		seen[path] = true
		standalone = append(standalone, path)
	}

	present := map[string]bool{}
	for _, l := range g.Syntax.Lines {
		for _, sp := range l.Spans {
			if sp.ImagePath != "" {
				present[sp.ImagePath] = true
			}
		}
	}

	var cmds []tea.Cmd
	for _, path := range standalone {
		absPath := m.resolveEmbed(path)
		if absPath == "" {
			continue
		}
		mtime := fileMtime(m.fs, absPath)

		if existing, ok := m.images[path]; ok {
			// Retry rule: Failed is sticky per (path, mtime) — an unchanged
			// mtime never respawns a failed instance (that would retry-storm
			// a permanently-broken file every discovery pass); a genuine
			// mtime change (e.g. `touch`, or a rewrite) always respawns and
			// retries, regardless of the prior state, including Failed.
			// PendingDecode also never respawns — a decode already in flight
			// for this path must run to completion before anything replaces it.
			if existing.Mtime() == mtime || existing.State() == image.PendingDecode {
				continue
			}
			// Free the outgoing generation's animation-frame pixel memory
			// NOW — the respawn reuses the base ID (its retransmit overwrites
			// that ID's terminal-side data in place) but never the frame IDs,
			// which otherwise leak in the terminal until despawn's DeleteCmd,
			// and that only covers the CURRENT instance's IDs. Frames only:
			// deleting the shared base ID here would race the new transmit
			// (see image.Model.DeleteFramesCmd). The allocator entries stay
			// until despawn's FreeAllForPath — conservative (no reuse, no
			// collision), and path-keyed so despawn frees every generation.
			if dcmd := existing.DeleteFramesCmd(); dcmd != nil {
				cmds = append(cmds, dcmd)
			}
			var cmd tea.Cmd
			m, cmd = m.spawnImage(path, absPath, existing.ID(), mtime, maxCols, maxRows)
			cmds = append(cmds, cmd)
			continue
		}

		id, na := m.idAlloc.AllocFreeID(absPath)
		m.idAlloc = na
		var cmd tea.Cmd
		m, cmd = m.spawnImage(path, absPath, id, mtime, maxCols, maxRows)
		cmds = append(cmds, cmd)
	}

	for path, img := range m.images {
		if present[path] {
			continue
		}
		delete(m.images, path)
		m.idAlloc = m.idAlloc.FreeAllForPath(img.AbsPath())
		if cmd := img.DeleteCmd(); cmd != nil {
			cmds = append(cmds, cmd)
		}
	}

	if len(cmds) == 0 {
		return m, nil
	}
	return m, tea.Batch(cmds...)
}

func (m Model) updateImages(msg tea.Msg) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	var path string

	switch msg := msg.(type) {
	case image.UpdateMsg:
		path = msg.Path
	case image.ReadyMsg:
		path = msg.Path
	case image.ErrorMsg:
		path = msg.Path
	}

	img, ok := m.images[path]
	if !ok {
		return m, nil
	}

	oldFailed := img.State() == image.Failed

	var cmd tea.Cmd
	img, cmd = img.Update(msg)
	if cmd != nil {
		cmds = append(cmds, cmd)
	}

	// The ¬Failed->Failed edge: surface the error once, on the existing
	// ImageErrorMsg channel the workspace already intercepts (E5) — otherwise
	// a decode/transmit/encode failure is invisible (pressure point #1).
	if !oldFailed && img.State() == image.Failed {
		err := img.Err()
		cmds = append(cmds, func() tea.Msg { return ImageErrorMsg{Err: err} })
	}

	if img.NeedsFrameIDs() {
		frameIDs, na := m.idAlloc.AllocFrameIDs(img.AbsPath(), img.FrameCount())
		m.idAlloc = na
		img, cmd = img.SetFrameIDs(frameIDs)
		if cmd != nil {
			cmds = append(cmds, cmd)
		}
	}

	m.images[path] = img

	// The image's layout footprint changes whenever its row count changes —
	// currentImageDims reserves rows for any decoded image (Height() > 1),
	// not just Live ones (fixed comment drift, pressure point #12: an older
	// version of this comment claimed "only Live images reserve expanded
	// rows", which currentImageDims's own doc comment already contradicts).
	// publishImageDimsIfChanged's dims-map comparison covers the change
	// exactly, and keeps m.publishedDims in sync so the next afterMutation
	// pass doesn't redundantly republish.
	m = m.publishImageDimsIfChanged()

	// Visibility + animation re-arm funnel through syncImageViewState (the
	// single re-arm chokepoint) here too: a lifecycle message is itself a
	// viewport-affecting change — decode expands the embed's reserved rows,
	// and the async ¬Live→Live ack is what makes an animated image eligible
	// to tick at all. Skipping this here left a freshly-decoded animated
	// image frozen at frame 0 until the next keypress/wheel/resize, because
	// its visibleRows still held the pre-decode projection (0) and nothing
	// else on the async path re-projected it (review finding).
	var vcmd tea.Cmd
	m, vcmd = m.syncImageViewState()
	if vcmd != nil {
		cmds = append(cmds, vcmd)
	}
	return m, tea.Batch(cmds...)
}

// mapImages applies fn to every tracked image, writing the result back and
// batching every non-nil Cmd. The one iteration site resizeImages,
// retransmitImagesCmd, detectImageCollapse, and syncImageViewState all share.
func (m Model) mapImages(fn func(path string, img image.Model) (image.Model, tea.Cmd)) (Model, tea.Cmd) {
	if len(m.images) == 0 {
		return m, nil
	}
	var cmds []tea.Cmd
	for path, img := range m.images {
		newImg, cmd := fn(path, img)
		m.images[path] = newImg
		if cmd != nil {
			cmds = append(cmds, cmd)
		}
	}
	if len(cmds) == 0 {
		return m, nil
	}
	return m, tea.Batch(cmds...)
}

// visibleRowsByPath reports, for every image path with at least one row in
// the current snapshot, how many of its display rows are currently within
// the viewport — one pass over the snapshot rather than one full-snapshot
// scan per image (the old visibleRowsFor, called once per image message).
func (m Model) visibleRowsByPath() map[string]int {
	g := m.Model.Geom()
	vp := g.Viewport
	contentH := g.ContentHeight
	counts := map[string]int{}
	for lineIdx, l := range g.Snap.Lines {
		if l.ImagePath == "" {
			continue
		}
		displayRow := lineIdx - vp.TopRow
		if displayRow >= 0 && displayRow < contentH {
			counts[l.ImagePath]++
		}
	}
	return counts
}

// syncImageViewState is THE single animation re-arm chokepoint: project
// visibility (one pass, visibleRowsByPath), then re-arm every image against
// the fresh count. Every path that can change what's on screen — keyboard
// scroll (via the reconcile funnel), mouse wheel, layout refresh, and the
// async image lifecycle messages themselves (updateImages: decode expands
// rows, the Live ack enables ticking) — routes through this one function, so
// none of them can forget the re-arm (the old scattered call sites: an
// inline arm in updateImages that missed keyboard scrolling entirely, and a
// since-deleted self-arm inside image.Model on the Kitty Live edge that
// consulted stale visibility).
func (m Model) syncImageViewState() (Model, tea.Cmd) {
	if !m.imageCapable() || len(m.images) == 0 {
		return m, nil
	}
	vis := m.visibleRowsByPath()
	return m.mapImages(func(path string, img image.Model) (image.Model, tea.Cmd) {
		img = img.SetVisibleRows(vis[path])
		return img.ArmTick()
	})
}

// retransmitImagesCmd re-transmits every image Resize just bumped back to
// PendingTransmit. Called from SetRect (folding what
// RefreshImagesAfterLayoutChange used to do as a separate workspace call).
func (m Model) retransmitImagesCmd() tea.Cmd {
	if !m.imageCapable() {
		return nil
	}
	_, cmd := m.mapImages(func(_ string, img image.Model) (image.Model, tea.Cmd) {
		return img, img.RetransmitCmd()
	})
	return cmd
}

// detectImageCollapse reconciles each tracked image's expanded state with the
// current snapshot and reports whether any image just collapsed (its rendered
// rows left the screen). The callers (afterContentChange / afterCursorMove)
// emit tea.ClearScreen on that edge to erase the now-stale pixels. The old
// version computed `collapsed` but returned a hardcoded false — the
// ClearScreen path was dead and only the placement-seq reset ran; the
// SetExpanded rewrite (which now returns the collapse edge directly instead
// of the caller diffing WasCollapsed() around the call) fixes that.
func (m Model) detectImageCollapse() (Model, bool) {
	if !m.imageCapable() || len(m.images) == 0 {
		return m, false
	}

	g := m.Model.Geom()
	expanded := make(map[string]bool)
	for _, l := range g.Snap.Lines {
		if l.ImagePath != "" && l.ImageRowIndex == 0 && l.ImageRowCount > 1 {
			expanded[l.ImagePath] = true
		}
	}

	collapsed := false
	m, _ = m.mapImages(func(path string, img image.Model) (image.Model, tea.Cmd) {
		newImg, justCollapsed := img.SetExpanded(expanded[path])
		if justCollapsed {
			collapsed = true
		}
		return newImg, nil
	})
	if collapsed {
		m.lastPlacementSeq = ""
	}
	return m, collapsed
}

// DeleteAllImagesCmd returns a command that deletes all images from the terminal.
func (m Model) DeleteAllImagesCmd() tea.Cmd {
	return image.DeleteAllCmd()
}

func (m Model) imageKittyCapable() bool {
	return m.termCaps.SupportsKittyGraphics()
}

func (m Model) imageInlineCapable() bool {
	return m.termCaps.SupportsInlineImages()
}

func (m Model) imageCapable() bool {
	return m.imageKittyCapable() || m.imageInlineCapable()
}

func fileMtime(fsys vfs.FS, absPath string) int64 {
	info, err := fsys.Stat(absPath)
	if err != nil {
		return 0
	}
	return info.ModTime().UnixNano()
}

func (m Model) resizeImages(maxCols, maxRows int) Model {
	if !m.imageCapable() || len(m.images) == 0 {
		return m
	}
	m, _ = m.mapImages(func(_ string, img image.Model) (image.Model, tea.Cmd) {
		newImg, changed := img.Resize(maxCols, maxRows)
		if !changed {
			return img, nil
		}
		return newImg, nil
	})
	return m
}

// currentImageDims builds the per-image cell footprints handed to the display
// pipeline. Images with a known cell size (decoded, transmitting, or live)
// reserve their full row count so the layout stays stable while pixels are in
// flight. Only PendingDecode (rows == 0) and Failed images stay at one row.
// The render layer (view.go) gates the actual Kitty placeholder on IsLive(),
// so no placeholder ever points at un-transmitted pixels (no black area). A
// nil result leaves every image line collapsed.
func (m Model) currentImageDims() map[string]display.ImageDims {
	if !m.imageCapable() || len(m.images) == 0 {
		return nil
	}
	dims := make(map[string]display.ImageDims, len(m.images))
	for path, img := range m.images {
		if img.Height() > 1 {
			dims[path] = display.ImageDims{Cols: img.Cols(), Rows: img.Height()}
		}
	}
	if len(dims) == 0 {
		return nil
	}
	return dims
}

// publishImageDimsIfChanged pushes currentImageDims into the display pipeline
// only when it actually differs from the last-published set (m.publishedDims),
// then follows with ScrollToCursor. SetImageDims runs a full display resync —
// gating it is what lets afterMutation call this on every cursor-only move
// without doing that work on every keypress (only genuine dims changes pay
// for it).
func (m Model) publishImageDimsIfChanged() Model {
	dims := m.currentImageDims()
	if maps.Equal(dims, m.publishedDims) {
		return m
	}
	m.publishedDims = dims
	m.Model = m.Model.SetImageDims(dims)
	m.Model = m.Model.ScrollToCursor()
	return m
}

// afterMutation is markdownedit's single post-mutation funnel (reconcile's
// doc comment promises this): every entry point that can change what's
// displayed — a buffer edit, a cursor-only move that flips reveal state, a
// layout change — ends here with contentChanged reporting which kind. Order
// matters: discovery must run before dims are republished (a freshly
// discovered image has no footprint yet), collapse detection reads the
// snapshot dims already reflect, and the animation re-arm must see the
// final, settled visibility.
func (m Model) afterMutation(contentChanged bool) (Model, tea.Cmd) {
	var cmds []tea.Cmd

	if contentChanged || m.hasUndiscoveredImages() {
		var dcmd tea.Cmd
		m, dcmd = m.syncImageSet()
		if dcmd != nil {
			cmds = append(cmds, dcmd)
		}
	}

	m = m.publishImageDimsIfChanged()

	var collapsed bool
	m, collapsed = m.detectImageCollapse()
	if collapsed {
		cmds = append(cmds, tea.ClearScreen)
	}

	var vcmd tea.Cmd
	m, vcmd = m.syncImageViewState()
	if vcmd != nil {
		cmds = append(cmds, vcmd)
	}

	return m, tea.Batch(cmds...)
}

// hasUndiscoveredImages reports whether the current snapshot contains any
// standalone-image path not yet tracked in m.images. Used to guard the
// per-cursor-move discovery call so steady-state navigation costs nothing.
func (m Model) hasUndiscoveredImages() bool {
	g := m.Model.Geom()
	for _, l := range g.Snap.Lines {
		path, ok := display.StandaloneImagePath(l)
		if !ok {
			continue
		}
		if _, tracked := m.images[path]; !tracked {
			return true
		}
	}
	return false
}

func (m Model) imageIDFor(path string) uint32 {
	if img, ok := m.images[path]; ok {
		return img.CurrentID()
	}
	return 0
}
