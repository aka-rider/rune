package editor

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/history"
	"rune/pkg/terminal"
)

// ImageConfig holds configuration for image paste handling.
type ImageConfig struct {
	AssetsDir string // relative directory for saved images (default "assets")
}

// ImageSavedMsg is produced when an image has been saved to disk.
type ImageSavedMsg struct {
	RelativePath string
}

// ImageSaveErrorMsg is produced when image saving fails.
type ImageSaveErrorMsg struct {
	Err error
}

// handleImagePaste processes a ClipboardContentMsg containing image data.
// It saves the image to the assets directory and inserts a markdown reference.
func (m Model) handleImagePaste(imgData []byte, mimeType string, now time.Time) (Model, tea.Cmd) {
	if len(imgData) == 0 {
		return m, nil
	}

	// Determine the base directory from the open file's location
	baseDir := m.imageBaseDir()
	if baseDir == "" {
		return m, nil
	}

	assetsDir := m.imageConfig.AssetsDir
	if assetsDir == "" {
		assetsDir = "assets"
	}

	// Validate assetsDir: reject absolute paths and path traversal
	if filepath.IsAbs(assetsDir) {
		return m, nil
	}
	if containsTraversal(assetsDir) {
		return m, nil
	}

	ext := extensionForMIME(mimeType)
	capturedData := imgData
	capturedAssetsDir := assetsDir
	capturedBaseDir := baseDir
	capturedExt := ext
	capturedNow := now

	cmd := func() tea.Msg {
		filename := generateImageFilename(capturedData, capturedNow, capturedExt)
		targetDir := filepath.Join(capturedBaseDir, capturedAssetsDir)

		if err := os.MkdirAll(targetDir, 0o755); err != nil {
			return ImageSaveErrorMsg{Err: fmt.Errorf("create assets dir %q: %w", targetDir, err)}
		}

		fullPath := filepath.Join(targetDir, filename)
		if err := os.WriteFile(fullPath, capturedData, 0o644); err != nil {
			return ImageSaveErrorMsg{Err: fmt.Errorf("write image %q: %w", fullPath, err)}
		}

		relativePath := filepath.Join(capturedAssetsDir, filename)
		return ImageSavedMsg{RelativePath: relativePath}
	}

	return m, cmd
}

// handleImageSaved inserts a markdown image reference at the cursor position.
func (m Model) handleImageSaved(relativePath string, now time.Time) (Model, tea.Cmd) {
	mdRef := fmt.Sprintf("![image](%s)", relativePath)
	return m.insertTextAtCursors(mdRef, now)
}

// insertTextAtCursors inserts text at all cursor positions.
func (m Model) insertTextAtCursors(text string, now time.Time) (Model, tea.Cmd) {
	all := m.cursors.All()
	if len(all) == 0 {
		return m, nil
	}

	type editInfo struct {
		edit buffer.Edit
		cID  int
	}

	var infos []editInfo
	for _, c := range all {
		if c.HasSelection() {
			start, end := c.SelectionRange()
			infos = append(infos, editInfo{
				edit: buffer.Edit{Start: start, End: end, Insert: text},
				cID:  c.ID,
			})
		} else {
			infos = append(infos, editInfo{
				edit: buffer.Edit{Start: c.Position, End: c.Position, Insert: text},
				cID:  c.ID,
			})
		}
	}

	// Sort descending by start
	for i := 0; i < len(infos)-1; i++ {
		for j := i + 1; j < len(infos); j++ {
			if infos[i].edit.Start < infos[j].edit.Start {
				infos[i], infos[j] = infos[j], infos[i]
			}
		}
	}

	edits := make([]buffer.Edit, len(infos))
	for i, info := range infos {
		edits[i] = info.edit
	}

	var newCursors []cursor.Cursor
	shift := 0
	for i := len(infos) - 1; i >= 0; i-- {
		info := infos[i]
		newPos := info.edit.Start + shift + len(text)
		newCursors = append(newCursors, cursor.Cursor{
			Position: newPos,
			Anchor:   newPos,
			ID:       info.cID,
		})
		shift += len(info.edit.Insert) - (info.edit.End - info.edit.Start)
	}

	op := command.Operation{
		Kind:    command.OperationEditBuffer,
		Edits:   edits,
		Cursors: cursor.NewCursorSetFrom(newCursors),
	}

	m = m.applyOperation(op, history.EditPaste, now)
	m = m.syncDisplay()
	return m, nil
}

// imageBaseDir returns the directory of the currently open file, or empty string.
func (m Model) imageBaseDir() string {
	if m.filePath == "" {
		return ""
	}
	return filepath.Dir(m.filePath)
}

// renderImageSpan returns a text fallback for an image display span.
func renderImageFallback(altText string, caps terminal.TermCaps) string {
	if caps.SupportsGraphics() {
		// Placeholder for future terminal graphics rendering.
		// For now, fall through to text fallback even with graphics support.
	}
	if altText == "" {
		altText = "image"
	}
	return fmt.Sprintf("[image: %s]", altText)
}

// generateImageFilename creates a collision-safe filename from timestamp + content hash.
func generateImageFilename(data []byte, now time.Time, ext string) string {
	hash := sha256.Sum256(data)
	shortHash := hex.EncodeToString(hash[:8])
	ts := now.Format("20060102-150405")
	return fmt.Sprintf("%s-%s%s", ts, shortHash, ext)
}

// extensionForMIME returns a file extension for the given MIME type.
func extensionForMIME(mime string) string {
	mime = strings.ToLower(mime)
	switch {
	case strings.Contains(mime, "png"):
		return ".png"
	case strings.Contains(mime, "jpeg"), strings.Contains(mime, "jpg"):
		return ".jpg"
	case strings.Contains(mime, "gif"):
		return ".gif"
	case strings.Contains(mime, "webp"):
		return ".webp"
	case strings.Contains(mime, "svg"):
		return ".svg"
	default:
		return ".png"
	}
}

// containsTraversal checks for path traversal attempts.
func containsTraversal(path string) bool {
	cleaned := filepath.Clean(path)
	if strings.HasPrefix(cleaned, "..") {
		return true
	}
	parts := strings.Split(cleaned, string(filepath.Separator))
	for _, p := range parts {
		if p == ".." {
			return true
		}
	}
	return false
}
