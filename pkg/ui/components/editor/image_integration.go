package editor

import (
	"fmt"
	"os"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/editor/display"
	"rune/pkg/ui/components/image"
)

const debugImageFile = "/tmp/rune_image_debug.log"

func debugLog(format string, args ...any) {
	f, err := os.OpenFile(debugImageFile, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0644)
	if err != nil {
		return
	}
	defer f.Close()
	fmt.Fprintf(f, "[%s] ", time.Now().Format("15:04:05.000"))
	fmt.Fprintf(f, format, args...)
	fmt.Fprintln(f)
}

func (m Model) emitImagePlacements() (Model, tea.Cmd) {
	if !m.imageInlineCapable() {
		return m, nil
	}
	seq := m.buildInlineImagePlacements()
	gated := seq == m.lastPlacementSeq

	debugLog("emitImagePlacements: seqLen=%d lastSeqLen=%d gated=%v topRow=%d offsetY=%d headerH=%d focused=%v pendingSeq=%d",
		len(seq), len(m.lastPlacementSeq), gated, m.viewport.TopRow, m.offsetY, m.headerHeight(), m.focused, len(m.pendingPlacementSeq))

	// If we have a pending sequence from last frame, emit it NOW.
	// This ensures the Raw arrives on a frame where BubbleTea's diff
	// has already written the spaces (no overwrite race).
	if m.pendingPlacementSeq != "" {
		pending := m.pendingPlacementSeq
		m.pendingPlacementSeq = ""
		m.lastPlacementSeq = pending
		debugLog("  → emitting PENDING Raw, seqLen=%d", len(pending))
		return m, tea.Raw(pending)
	}

	if gated {
		return m, nil
	}

	if seq == "" {
		m.lastPlacementSeq = ""
		debugLog("  → seq empty, clearing lastPlacementSeq")
		return m, nil
	}

	// Don't emit now — defer to next frame so BubbleTea's diff renderer
	// writes the spaces first, then our Raw paints over them.
	m.pendingPlacementSeq = seq
	debugLog("  → deferring seq to next frame, seqLen=%d", len(seq))
	// Return a no-op cmd that triggers another Update cycle
	return m, func() tea.Msg { return placementTickMsg{} }
}

// placementTickMsg is an internal message that triggers the next Update cycle
// so the deferred placement sequence can be emitted.
type placementTickMsg struct{}

func (m Model) buildInlineImagePlacements() string {
	topRow := m.viewport.TopRow
	contentH := m.contentHeight()
	screenBase := m.offsetY + m.headerHeight()
	col := m.offsetX + 2 // 1-based terminal column + 1 left margin

	debugLog("buildInlineImagePlacements: topRow=%d contentH=%d screenBase=%d col=%d snapshotLines=%d",
		topRow, contentH, screenBase, col, len(m.snapshot.Lines))

	var sb strings.Builder
	for lineIdx, l := range m.snapshot.Lines {
		if l.ImagePath == "" {
			continue
		}
		img, ok := m.images[l.ImagePath]
		if !ok || !img.IsLive() || len(img.ITerm2Slices()) == 0 {
			debugLog("  line[%d] path=%q skip: ok=%v live=%v slices=%d",
				lineIdx, l.ImagePath, ok, ok && img.IsLive(), func() int { if ok { return len(img.ITerm2Slices()) }; return 0 }())
			continue
		}

		// Only process rows that fall within the visible viewport.
		displayRow := lineIdx - topRow
		if displayRow < 0 || displayRow >= contentH {
			continue
		}

		slices := img.ITerm2Slices()
		if l.ImageRowIndex < 0 || l.ImageRowIndex >= len(slices) {
			debugLog("  line[%d] path=%q rowIdx=%d OUT OF RANGE (slices=%d)", lineIdx, l.ImagePath, l.ImageRowIndex, len(slices))
			continue
		}

		screenRow := screenBase + displayRow + 1 // +1 for 1-based terminal rows
		if l.ImageRowIndex == 0 {
			debugLog("  FIRST ROW: line[%d] path=%q displayRow=%d screenRow=%d rowIdx=%d totalSlices=%d imgRows=%d",
				lineIdx, l.ImagePath, displayRow, screenRow, l.ImageRowIndex, len(slices), img.Height())
		}
		fmt.Fprintf(&sb, "\0337\033[%d;%dH%s\0338", screenRow, col, slices[l.ImageRowIndex])
	}

	return sb.String()
}

func (m Model) discoverNewImages() (Model, tea.Cmd) {
	if !m.imageCapable() {
		return m, nil
	}
	maxCols := m.imageMaxCols()
	maxRows := m.contentHeight()
	cs := m.cellSize

	seen := map[string]bool{}
	var cmds []tea.Cmd
	for _, l := range m.snapshot.Lines {
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
			// need to re-decode, but we should create a new model so we get a fresh state
			// keep the same id though
			id := existing.ID() // Wait, image.Model doesn't expose ID()? I need to add that.
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
	// gather ids... need to expose ID and frameIDs from model.
	// Oh wait, DeleteAllImagesCmd is global, we don't need to pass specific IDs except for cleanup.
	// We'll write to DeleteCmd.
	// But it's easier to just call image.DeleteCmd() without ids?
	// The original code used DeleteImagesCmd(ids), but the workspace calls DeleteAllImagesCmd().
	// We'll use DeleteAllImagesCmd() for this too since we are clearing everything.
	// Wait, clearImages() used DeleteImagesCmd(ids). I should get IDs.
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

	if msgErr, isErr := msg.(image.ErrorMsg); isErr {
		// Log error somewhere?
		_ = msgErr
		img = img.MarkFailed()
	}

	// Handle frame IDs allocation if needed
	if img.NeedsFrameIDs() {
		// Alloc frame IDs
		frameIDs, na := m.idAlloc.AllocFrameIDs(img.AbsPath(), img.FrameCount())
		m.idAlloc = na
		img, cmd = img.SetFrameIDs(frameIDs)
		if cmd != nil {
			cmds = append(cmds, cmd)
		}
	}

	m.images[path] = img

	if img.Height() != oldHeight {
		m = m.syncDisplay()
		m = m.scrollToCursor()
	}
	if !oldLive && img.IsLive() {
		// Just became live: we might need to arm ticks if animated
		var armCmd tea.Cmd
		img, armCmd = img.ArmTick()
		m.images[path] = img
		if armCmd != nil {
			cmds = append(cmds, armCmd)
		}
	}

	return m, tea.Batch(cmds...)
}

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

	expanded := make(map[string]bool)
	for _, l := range m.snapshot.Lines {
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
			debugLog("detectImageCollapse: path=%q COLLAPSED (wasExpanded→!expanded)", path)
		}
		m.images[path] = img
	}
	if collapsed {
		debugLog("detectImageCollapse: resetting lastPlacementSeq (NO ClearScreen)")
		m.lastPlacementSeq = ""
	}
	// NOTE: We intentionally do NOT emit tea.ClearScreen here.
	// ClearScreen races with tea.Raw placement on re-expansion.
	// Instead, the normal View() repaint handles erasure — when image rows
	// collapse, BubbleTea's diff renderer writes text/spaces over the area,
	// naturally clearing the image pixels.
	return m, false // always false — caller should not emit ClearScreen
}

func (m Model) DeleteAllImagesCmd() tea.Cmd {
	return image.DeleteAllCmd()
}

// imageKittyCapable reports whether inline image rendering is active for this terminal.
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

func (m Model) imageDimsFor(path string) display.ImageDims {
	if !m.imageCapable() {
		return display.ImageDims{Cols: 0, Rows: 1}
	}
	img, ok := m.images[path]
	if !ok || img.Height() <= 1 {
		return display.ImageDims{Cols: 0, Rows: 1}
	}
	return display.ImageDims{Cols: img.Cols(), Rows: img.Height()}
}

func (m Model) imageIDFor(path string) uint32 {
	if img, ok := m.images[path]; ok {
		return img.CurrentID()
	}
	return 0
}
