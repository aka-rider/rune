package markdownedit

import (
	"fmt"
	"os"
	"strings"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/editor/display"
	"rune/pkg/ui/components/image"
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
	snap := m.Model.Snapshot()
	vp := m.Model.Viewport()
	contentH := m.Model.ContentHeight()
	screenBase := m.Model.OffsetY()
	col := m.Model.OffsetX() + 2 // 1-based terminal column + 1 left margin

	var place strings.Builder
	regions := map[string]placedRegion{}

	for lineIdx, l := range snap.Lines {
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

func (m Model) discoverNewImages() (Model, tea.Cmd) {
	if !m.imageCapable() {
		return m, nil
	}
	maxCols := m.Model.ImageMaxCols()
	maxRows := m.Model.ContentHeight()
	cs := m.cellSize

	snap := m.Model.Snapshot()
	seen := map[string]bool{}
	var cmds []tea.Cmd
	for _, l := range snap.Lines {
		path, ok := display.StandaloneImagePath(l)
		if !ok || seen[path] {
			continue
		}
		seen[path] = true

		absPath := m.resolveEmbed(path)
		if absPath == "" {
			continue
		}
		mtime := fileMtime(absPath)

		if existing, ok := m.images[path]; ok {
			if existing.Mtime() == mtime || existing.State() == image.PendingDecode || existing.State() == image.Failed {
				continue
			}
			id := existing.ID()
			newImg := image.New(path, absPath, id, mtime, m.termCaps, cs, maxCols, maxRows)
			m.images[path] = newImg
			cmds = append(cmds, newImg.Init())
			continue
		}

		id, na := m.idAlloc.AllocFreeID(absPath)
		m.idAlloc = na
		newImg := image.New(path, absPath, id, mtime, m.termCaps, cs, maxCols, maxRows)
		m.images[path] = newImg
		cmds = append(cmds, newImg.Init())
	}

	if len(cmds) == 0 {
		return m, nil
	}
	return m, tea.Batch(cmds...)
}

func (m Model) clearImages() (Model, tea.Cmd) {
	var ids []uint32
	for _, img := range m.images {
		ids = append(ids, img.LiveIDs()...)
	}
	m.images = make(map[string]image.Model)
	if len(ids) == 0 {
		return m, nil
	}
	return m, image.DeleteCmd(ids)
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

	oldHeight := img.Height()
	oldLive := img.IsLive()

	var cmd tea.Cmd
	img, cmd = img.Update(msg)
	if cmd != nil {
		cmds = append(cmds, cmd)
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

	// The image's layout footprint changes when its row count changes or when
	// it transitions to/from Live (only Live images reserve expanded rows).
	// Push the new footprints into the display pipeline so the snapshot — and
	// the scroll math that depends on it — stay consistent.
	if img.Height() != oldHeight || img.IsLive() != oldLive {
		m.Model = m.Model.SetImageDims(m.currentImageDims())
		m.Model = m.Model.ScrollToCursor()
	}

	// Tell the image how many of its rows are on-screen (computed against the
	// now-expanded snapshot). Animation ticking is gated on visibleRows > 0, so
	// without this an animated image never advances past frame 0. Must run
	// before ArmTick below.
	img = img.SetVisibleRows(m.visibleRowsFor(path))
	m.images[path] = img

	if !oldLive && img.IsLive() {
		var armCmd tea.Cmd
		img, armCmd = img.ArmTick()
		m.images[path] = img
		if armCmd != nil {
			cmds = append(cmds, armCmd)
		}
	}

	return m, tea.Batch(cmds...)
}

// visibleRowsFor reports how many display rows of the given image are currently
// within the viewport, using the same geometry as buildInlineImagePlacements.
// Drives animation gating: an off-screen (or cursor-revealed-to-source) image
// reports 0 and pauses its ticks.
func (m Model) visibleRowsFor(path string) int {
	snap := m.Model.Snapshot()
	vp := m.Model.Viewport()
	contentH := m.Model.ContentHeight()
	count := 0
	for lineIdx, l := range snap.Lines {
		if l.ImagePath != path {
			continue
		}
		displayRow := lineIdx - vp.TopRow
		if displayRow >= 0 && displayRow < contentH {
			count++
		}
	}
	return count
}

// RefreshImagesAfterLayoutChange retransmits and re-arms image ticks.
// Called by workspace after SetRect.
func (m Model) RefreshImagesAfterLayoutChange() (Model, tea.Cmd) {
	var cmds []tea.Cmd
	if cmd := m.retransmitImagesCmd(); cmd != nil {
		cmds = append(cmds, cmd)
	}
	var cmd tea.Cmd
	m, cmd = m.armImageTicks()
	if cmd != nil {
		cmds = append(cmds, cmd)
	}
	if len(cmds) > 0 {
		return m, tea.Batch(cmds...)
	}
	return m, nil
}

func (m Model) retransmitImagesCmd() tea.Cmd {
	if !m.imageCapable() {
		return nil
	}
	var cmds []tea.Cmd
	for _, img := range m.images {
		if cmd := img.RetransmitCmd(); cmd != nil {
			cmds = append(cmds, cmd)
		}
	}
	if len(cmds) == 0 {
		return nil
	}
	return tea.Batch(cmds...)
}

func (m Model) armImageTicks() (Model, tea.Cmd) {
	if !m.imageCapable() {
		return m, nil
	}
	var cmds []tea.Cmd
	for path, img := range m.images {
		newImg, cmd := img.ArmTick()
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

func (m Model) detectImageCollapse() (Model, bool) {
	if !m.imageCapable() || len(m.images) == 0 {
		return m, false
	}

	snap := m.Model.Snapshot()
	expanded := make(map[string]bool)
	for _, l := range snap.Lines {
		if l.ImagePath != "" && l.ImageRowIndex == 0 && l.ImageRowCount > 1 {
			expanded[l.ImagePath] = true
		}
	}

	collapsed := false
	for path, img := range m.images {
		wasCollapsed := img.WasCollapsed()
		img = img.SetExpanded(expanded[path])
		if img.WasCollapsed() && !wasCollapsed {
			collapsed = true
		}
		m.images[path] = img
	}
	if collapsed {
		m.lastPlacementSeq = ""
	}
	return m, false
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

func fileMtime(absPath string) int64 {
	info, err := os.Stat(absPath)
	if err != nil {
		return 0
	}
	return info.ModTime().UnixNano()
}

func (m Model) resizeImages(maxCols, maxRows int) Model {
	if !m.imageCapable() || len(m.images) == 0 {
		return m
	}
	for path, img := range m.images {
		newImg, changed := img.Resize(maxCols, maxRows)
		if changed {
			m.images[path] = newImg
		}
	}
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

// hasUndiscoveredImages reports whether the current snapshot contains any
// standalone-image path not yet tracked in m.images. Used to guard the
// per-cursor-move discovery call so steady-state navigation costs nothing.
func (m Model) hasUndiscoveredImages() bool {
	snap := m.Model.Snapshot()
	for _, l := range snap.Lines {
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
