package editor

import (
	"os"
	"path/filepath"
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
	baseDir := m.imageBaseDir()
	maxCols := m.width
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

		absPath := m.resolveImagePath(path, baseDir)
		if absPath == "" {
			continue
		}
		mtime := fileMtime(absPath)

		if existing, ok := m.images.get(path); ok {
			// Already tracked — only re-decode if the source changed on disk.
			if existing.mtime == mtime || existing.state == pendingDecode {
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

// handleImageEncoded stores the iTerm2 payload and dispatches initial placement.
func (m Model) handleImageEncoded(msg ImageEncodedMsg) (Model, tea.Cmd) {
	e, ok := m.images.get(msg.Path)
	if !ok {
		return m, nil
	}
	e.iterm2Payload = msg.Payload
	e.state = live
	e.lastScreenRow = -1 // not yet placed
	m.images = m.images.upsert(e)
	return m, m.replotInlineImages()
}

// replotInlineImages emits PlaceITerm2Cmd for each live iTerm2 image whose
// screen position changed or that hasn't been placed yet.
func (m Model) replotInlineImages() tea.Cmd {
	if !m.imageInlineCapable() {
		return nil
	}
	topRow := m.viewport.TopRow
	contentH := m.contentHeight()
	// offsetY is the vertical offset from the top of the editor widget to the
	// first content row on screen (accounts for breadcrumb etc.).
	// In the terminal, row 1 is the top. We need the absolute screen row,
	// but since we write to TTY with cursor positioning we use the viewport-
	// relative row offset from editor's screen origin.
	screenBase := m.offsetY + m.breadcrumb.Height()

	var cmds []tea.Cmd
	for lineIdx, l := range m.snapshot.Lines {
		if l.ImagePath == "" || l.ImageRowIndex != 0 {
			continue // only process anchor rows
		}
		e, ok := m.images.get(l.ImagePath)
		if !ok || e.state != live || e.iterm2Payload == "" {
			continue
		}
		// Compute screen row (1-based terminal row).
		displayRow := lineIdx - topRow
		if displayRow < 0 || displayRow >= contentH {
			// Image is outside visible viewport — skip.
			continue
		}
		screenRow := screenBase + displayRow + 1 // +1 for 1-based terminal rows
		if screenRow == e.lastScreenRow {
			continue // already placed at this position
		}
		e.lastScreenRow = screenRow
		m.images = m.images.upsert(e)

		col := m.offsetX + 1 // 1-based terminal column
		cmds = append(cmds, PlaceITerm2Cmd(e.path, e.iterm2Payload, screenRow, col))
	}
	if len(cmds) == 0 {
		return nil
	}
	return tea.Batch(cmds...)
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
		e.iterm2Payload = "" // invalidate cached payload — dimensions changed
		e.lastScreenRow = -1
		if e.state == live {
			e.state = pendingTransmit
		}
		m.images = m.images.upsert(e)
	}
	return m
}

// resolveImagePath resolves a raw markdown destination to an absolute on-disk
// path, or "" for remote/data URLs or unresolvable relative paths.
func (m Model) resolveImagePath(path, baseDir string) string {
	if path == "" || isRemoteURL(path) {
		return ""
	}
	if filepath.IsAbs(path) {
		return filepath.Clean(path)
	}
	if baseDir == "" {
		return ""
	}
	return filepath.Join(baseDir, path)
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
		if sp.Kind == display.TokenImage {
			if sp.AltText != "" {
				return sp.AltText
			}
			return sp.Text
		}
	}
	return ""
}
