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

// Clipboard messages for two-phase async paste.

type ClipboardContentMsg struct {
	Text      string
	ImageData []byte
	MIMEType  string
}

type ClipboardErrorMsg struct {
	Err error
}

type ClipboardWrittenMsg struct{}
