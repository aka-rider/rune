package editor

import (
	"fmt"
	"os"
	"strings"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/editor/display"
	"rune/pkg/imagekit"
)

// discoverNewImages scans the current snapshot for standalone image lines whose
// images are not yet tracked (or whose source changed on disk) and dispatches a
// DecodeImageCmd for each. It is a no-op on non-capable terminals. The returned
// Cmd batches all decodes; the Model gains pendingDecode registry entries.
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

		if existing, ok := m.images.get(path); ok {
			// Already tracked — only re-decode if the source changed on disk. A
			// failed entry is not retried every sync unless its mtime changed.
			if existing.mtime == mtime || existing.state == pendingDecode || existing.state == failed {
				continue
			}
			existing.state = pendingDecode
			existing.mtime = mtime
			m.images = m.images.upsert(existing)
			cmds = append(cmds, DecodeImageCmd(path, absPath, mtime, maxCols, maxRows, cs))
			continue
		}

		id := m.images.allocFreeID(absPath)
		m.images = m.images.upsert(imageEntry{
			path:    path,
			absPath: absPath,
			id:      id,
			mtime:   mtime,
			state:   pendingDecode,
			altText: imageAltFromLine(l),
		})
		cmds = append(cmds, DecodeImageCmd(path, absPath, mtime, maxCols, maxRows, cs))
	}

	if len(cmds) == 0 {
		return m, nil
	}
	return m, tea.Batch(cmds...)
}

// handleImageDecoded records measured dimensions, reserves rows, re-anchors the
// viewport, and dispatches the transmit (Kitty) or encode (iTerm2).
func (m Model) handleImageDecoded(msg ImageDecodedMsg) (Model, tea.Cmd) {
	e, ok := m.images.get(msg.Path)
	if !ok {
		// Entry was removed (file switched/closed) before decode finished.
		return m, nil
	}
	e.cols = msg.Cols
	e.rows = msg.Rows
	e.pxW = msg.PxW
	e.pxH = msg.PxH
	e.mtime = msg.Mtime
	e.state = pendingTransmit

	if msg.Animated && msg.FrameCount > 1 && m.imageKittyCapable() {
		e.animated = true
		e.frameCount = msg.FrameCount
		e.delays = msg.Delays
		e.loopCount = msg.LoopCount
		e.frameIdx = 0
		e.loopsDone = 0
		e.frameIDs = m.images.allocFrameIDs(e.absPath, msg.FrameCount)
		m.images = m.images.upsert(e)
		m = m.syncDisplay()
		m = m.scrollToCursor()
		return m, TransmitAnimationCmd(e.path, e.absPath, e.frameIDs, e.cols, e.rows, m.cellSize)
	}

	m.images = m.images.upsert(e)
	m = m.syncDisplay()    // rows now reserve
	m = m.scrollToCursor() // re-anchor against expanded rows

	if m.imageKittyCapable() {
		return m, TransmitImageCmd(e.path, e.absPath, e.id, e.cols, e.rows, m.cellSize)
	}
	// iTerm2/WezTerm path: encode to OSC 1337 payload.
	return m, EncodeITerm2Cmd(e.path, e.absPath, e.cols, e.rows, m.cellSize)
}

// handleImageTransmitted marks an image live.
func (m Model) handleImageTransmitted(msg ImageTransmittedMsg) (Model, tea.Cmd) {
	e, ok := m.images.get(msg.Path)
	if !ok {
		return m, nil
	}
	e.state = live
	m.images = m.images.upsert(e)
	if e.animated && e.frameCount > 1 {
		// Begin cycling frames now that all frames are transmitted.
		return m.armImageTicks()
	}
	return m, nil
}

// handleImageEncoded stores the iTerm2 row-slices. The next View() call will
// automatically place the image at the correct screen position.
func (m Model) handleImageEncoded(msg ImageEncodedMsg) (Model, tea.Cmd) {
	e, ok := m.images.get(msg.Path)
	if !ok {
		return m, nil
	}
	e.iterm2Slices = msg.Slices
	e.state = live
	m.images = m.images.upsert(e)
	return m, nil
}

// buildInlineImagePlacements builds a single escape sequence containing all visible
// iTerm2 row-slice placements. Called from View() to embed placements atomically
// in the same frame as the text render. Returns the raw escape string (no I/O).
func (m Model) buildInlineImagePlacements() string {
	if !m.imageInlineCapable() {
		return ""
	}
	topRow := m.viewport.TopRow
	contentH := m.contentHeight()
	screenBase := m.offsetY + m.headerHeight()
	col := m.offsetX + 2 // 1-based terminal column + 1 left margin

	var sb strings.Builder
	for lineIdx, l := range m.snapshot.Lines {
		if l.ImagePath == "" {
			continue
		}
		e, ok := m.images.get(l.ImagePath)
		if !ok || e.state != live || len(e.iterm2Slices) == 0 {
			continue
		}

		// Only process rows that fall within the visible viewport.
		displayRow := lineIdx - topRow
		if displayRow < 0 || displayRow >= contentH {
			continue
		}

		// Bounds-check the slice index.
		if l.ImageRowIndex < 0 || l.ImageRowIndex >= len(e.iterm2Slices) {
			continue
		}

		screenRow := screenBase + displayRow + 1 // +1 for 1-based terminal rows
		// DECSC (\0337) / DECRC (\0338) save and restore the cursor; WezTerm
		// honors these consistently where the SCO save/restore (\033[s/\033[u)
		// is unreliable.
		fmt.Fprintf(&sb, "\0337\033[%d;%dH%s\0338", screenRow, col, e.iterm2Slices[l.ImageRowIndex])
	}

	return sb.String()
}

// emitInlinePlacements emits the current iTerm2/WezTerm inline-image placement
// escapes via tea.Raw (written straight to the tty, bypassing the cell
// renderer) — but only when the visible placement set changed since the last
// emit, to avoid per-frame flicker.
func (m Model) emitInlinePlacements() (Model, tea.Cmd) {
	if !m.imageInlineCapable() {
		return m, nil
	}
	seq := m.buildInlineImagePlacements()
	if seq == m.lastPlacementSeq {
		return m, nil
	}
	m.lastPlacementSeq = seq
	if seq == "" {
		return m, nil // nothing to paint; frame redraw clears any prior image rows
	}
	return m, tea.Raw(seq)
}

// handleImageError marks an image failed so it collapses back to the alt-text
// fallback. Used for both decode and transmit failures.
func (m Model) handleImageError(path string) (Model, tea.Cmd) {
	e, ok := m.images.get(path)
	if !ok {
		return m, nil
	}
	e.state = failed
	m.images = m.images.upsert(e)
	m = m.syncDisplay() // collapse the reserved rows back to one
	return m, nil
}

// detectImageCollapse checks whether any previously-expanded image is no longer
// expanded in the current snapshot (i.e., it collapsed because the cursor
// entered the tag). Returns true if at least one collapse occurred, and updates
// the wasExpanded flag on all registry entries.
func (m Model) detectImageCollapse() (Model, bool) {
	if !m.imageCapable() || len(m.images.byPath) == 0 {
		return m, false
	}

	// Build set of currently expanded image paths from the snapshot.
	expanded := make(map[string]bool)
	for _, l := range m.snapshot.Lines {
		if l.ImagePath != "" && l.ImageRowIndex == 0 && l.ImageRowCount > 1 {
			expanded[l.ImagePath] = true
		}
	}

	collapsed := false
	for _, e := range m.images.byPath {
		nowExpanded := expanded[e.path]
		if e.wasExpanded && !nowExpanded {
			collapsed = true
		}
		if e.wasExpanded != nowExpanded {
			e.wasExpanded = nowExpanded
			m.images = m.images.upsert(e)
		}
	}
	return m, collapsed
}

// clearImages deletes all tracked images from the terminal and resets the
// registry. Returned Cmd is fire-and-forget.
func (m Model) clearImages() (Model, tea.Cmd) {
	ids := m.images.liveIDs()
	m.images = newImageRegistry()
	if len(ids) == 0 {
		return m, nil
	}
	return m, DeleteImagesCmd(ids)
}

// retransmitImagesCmd re-sends every image with known dimensions at its current
// cell footprint. Used after a resize, when the cell box (cols/rows) changed.
func (m Model) retransmitImagesCmd() tea.Cmd {
	if !m.imageCapable() {
		return nil
	}
	cs := m.cellSize
	var cmds []tea.Cmd
	for _, e := range m.images.byPath {
		if e.state != pendingTransmit && e.state != live {
			continue
		}
		if e.cols <= 0 || e.rows <= 0 {
			continue
		}
		if m.imageKittyCapable() {
			cmds = append(cmds, TransmitImageCmd(e.path, e.absPath, e.id, e.cols, e.rows, cs))
		} else {
			cmds = append(cmds, EncodeITerm2Cmd(e.path, e.absPath, e.cols, e.rows, cs))
		}
	}
	if len(cmds) == 0 {
		return nil
	}
	return tea.Batch(cmds...)
}

// resizeImages recomputes each tracked image's cell footprint for new editor
// dimensions and marks changed entries pendingTransmit. Called from SetSize on
// an actual dimension change. Pure metadata work — no decode, no I/O.
func (m Model) resizeImages(maxCols, maxRows int) Model {
	if !m.imageCapable() || len(m.images.byPath) == 0 {
		return m
	}
	for _, e := range m.images.byPath {
		if e.pxW <= 0 || e.pxH <= 0 {
			continue
		}
		cols, rows := imagekit.FitCells(e.pxW, e.pxH, maxCols, maxRows, m.cellSize)
		if cols == e.cols && rows == e.rows {
			continue
		}
		e.cols = cols
		e.rows = rows
		e.iterm2Slices = nil // invalidate cached slices — dimensions changed
		if e.state == live {
			e.state = pendingTransmit
		}
		m.images = m.images.upsert(e)
	}
	return m
}

func isRemoteURL(path string) bool {
	low := strings.ToLower(path)
	return strings.HasPrefix(low, "http://") ||
		strings.HasPrefix(low, "https://") ||
		strings.HasPrefix(low, "data:")
}

// fileMtime returns the file's modtime in nanoseconds, or 0 if unavailable.
func fileMtime(absPath string) int64 {
	info, err := os.Stat(absPath)
	if err != nil {
		return 0
	}
	return info.ModTime().UnixNano()
}

// imageAltFromLine extracts the alt text from a standalone image line, for the
// failure fallback.
func imageAltFromLine(l display.DisplayLine) string {
	for _, sp := range l.Spans {
		if sp.Kind == display.TokenImage || (sp.Kind == display.TokenWikiLink && sp.WikiLinkIsImage) {
			if sp.AltText != "" {
				return sp.AltText
			}
			return sp.Text
		}
	}
	return ""
}

// RefreshImagesAfterLayoutChange batches retransmitImagesCmd() and armImageTicks()
// after non-terminal layout changes such as pane divider drags. Inline image
// placement is handled automatically by View().
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
