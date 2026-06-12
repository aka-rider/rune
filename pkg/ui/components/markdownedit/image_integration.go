package markdownedit

import (
	"fmt"
	"os"
	"strings"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/editor/display"
	"rune/pkg/ui/components/image"
)

func (m Model) emitImagePlacements() (Model, tea.Cmd) {
	if !m.imageInlineCapable() {
		return m, nil
	}
	seq := m.buildInlineImagePlacements()
	if seq == m.lastPlacementSeq {
		return m, nil
	}
	m.lastPlacementSeq = seq
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

func (m Model) buildInlineImagePlacements() string {
	snap := m.Model.Snapshot()
	vp := m.Model.Viewport()
	contentH := m.Model.ContentHeight()
	screenBase := m.Model.OffsetY()
	col := m.Model.OffsetX() + 2 // 1-based terminal column + 1 left margin

	var sb strings.Builder
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
		fmt.Fprintf(&sb, "\0337\033[%d;%dH%s\0338", screenRow, col, slices[l.ImageRowIndex])
	}

	return sb.String()
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

	if img.Height() != oldHeight {
		snap := m.Model.Snapshot()
		snap = display.ExpandImageRows(snap, m.imageDimsFor)
		m.Model = m.Model.SetSnapshot(snap)
		m.Model = m.Model.ScrollToCursor()
	}
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
