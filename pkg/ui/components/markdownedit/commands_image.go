package markdownedit

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"path/filepath"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"
)

// ImageConfig holds configuration for image paste handling.
type ImageConfig struct {
	AssetsDir string // relative directory for saved images (default "assets")
}

func (m Model) handleImagePaste(imgData []byte, mimeType string, now time.Time) (Model, tea.Cmd) {
	if len(imgData) == 0 {
		return m, nil
	}

	baseDir := m.docDir() // pasted images save next to the note
	if baseDir == "" {
		baseDir = m.root // untitled (no folder of its own): fall back to the workspace root
	}
	if baseDir == "" {
		return m, nil // no resolvable location at all
	}

	assetsDir := m.imageConfig.AssetsDir
	if assetsDir == "" {
		assetsDir = "assets"
	}

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
	capturedFsys := m.fsys() // §1.4.9: write through the editor's FS (Disk→atomicfile)

	cmd := func() tea.Msg {
		filename := generateImageFilename(capturedData, capturedNow, capturedExt)
		targetDir := filepath.Join(capturedBaseDir, capturedAssetsDir)

		if err := capturedFsys.MkdirAll(targetDir, 0o755); err != nil {
			return ImageSaveErrorMsg{Err: fmt.Errorf("create assets dir %q: %w", targetDir, err)}
		}

		fullPath := filepath.Join(targetDir, filename)
		if err := capturedFsys.WriteFile(fullPath, capturedData, 0o644); err != nil {
			return ImageSaveErrorMsg{Err: fmt.Errorf("write image %q: %w", fullPath, err)}
		}

		relativePath := filepath.Join(capturedAssetsDir, filename)
		return ImageSavedMsg{RelativePath: relativePath}
	}

	return m, cmd
}

func (m Model) handleImageSaved(relativePath string, now time.Time) (Model, tea.Cmd) {
	mdRef := fmt.Sprintf("![image](%s)", relativePath)
	return m.insertTextAtCursors(mdRef, now)
}

func (m Model) insertTextAtCursors(text string, now time.Time) (Model, tea.Cmd) {
	_ = now
	if len(m.Model.CursorOffsets()) == 0 {
		return m, nil
	}

	var err error
	sels := m.Model.Selections()
	if len(sels) > 0 {
		m.Model, err = m.Model.ReplaceRange(sels[0].Start, sels[0].End, text)
	} else {
		offset := m.Model.CursorOffset()
		m.Model, err = m.Model.ReplaceRange(offset, offset, text)
	}
	if err != nil {
		// §1.3: the buffer is left unchanged (ReplaceRange's guarantee) — surface
		// on the existing image-error channel rather than silently proceed as if
		// the image reference had been inserted.
		return m, func() tea.Msg { return ImageSaveErrorMsg{Err: err} }
	}

	return m.afterContentChange()
}

func generateImageFilename(data []byte, now time.Time, ext string) string {
	hash := sha256.Sum256(data)
	shortHash := hex.EncodeToString(hash[:8])
	ts := now.Format("20060102-150405")
	return fmt.Sprintf("%s-%s%s", ts, shortHash, ext)
}

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
