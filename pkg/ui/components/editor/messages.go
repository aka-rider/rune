package editor

type FileLoadedMsg struct {
	Path    string
	Content []byte
}

type FileLoadErrorMsg struct {
	Path string
	Err  error
}

type FileClosedMsg struct {
	Path string
}

type FileSavedMsg struct {
	Path             string
	RequestID        string
	SavedContentHash string
}

type FileSaveErrorMsg struct {
	Path      string
	RequestID string
	Err       error
}

type ContentChangedMsg struct {
	Path  string
	Dirty bool
}

// ClipboardContentMsg is used for image paste (carries binary image data).
// Text paste from the system clipboard arrives as tea.ClipboardMsg instead.

type ClipboardContentMsg struct {
	Text      string
	ImageData []byte
	MIMEType  string
}

// LinkClickedMsg is emitted when the user clicks on a wiki link or markdown link.
type LinkClickedMsg struct {
	Path string // resolved file path (empty for external URLs)
}

type FileRenamedMsg struct {
	OldPath string
	NewPath string
}

type FileRenameErrorMsg struct {
	OldPath string
	Err     error
}
