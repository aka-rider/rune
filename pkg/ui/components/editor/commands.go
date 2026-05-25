package editor

import (
	"fmt"
	"os"

	tea "charm.land/bubbletea/v2"
)

type SaveRequest struct {
	Path        string
	Content     string
	RequestID   string
	ContentHash string
}

func LoadFileCmd(path string) tea.Cmd {
	return func() tea.Msg {
		b, err := os.ReadFile(path)
		if err != nil {
			return FileLoadErrorMsg{Path: path, Err: fmt.Errorf("open %q: %w", path, err)}
		}
		return FileLoadedMsg{Path: path, Content: b}
	}
}

func SaveFileCmd(req SaveRequest) tea.Cmd {
	return func() tea.Msg {
		err := os.WriteFile(req.Path, []byte(req.Content), 0644)
		if err != nil {
			return FileSaveErrorMsg{Path: req.Path, RequestID: req.RequestID, Err: err}
		}
		return FileSavedMsg{
			Path:             req.Path,
			RequestID:        req.RequestID,
			SavedContentHash: req.ContentHash,
		}
	}
}
