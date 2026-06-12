package textedit

// ClipboardContentMsg is used for image paste (carries binary image data).
// Text paste from the system clipboard arrives as tea.ClipboardMsg instead.
type ClipboardContentMsg struct {
	Text      string
	ImageData []byte
	MIMEType  string
}
