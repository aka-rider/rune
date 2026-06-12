package markdownedit

// LinkClickedMsg is emitted when the user clicks a wiki link or markdown link.
type LinkClickedMsg struct {
	Path string // resolved file path (empty for external URLs)
}

// ImageSavedMsg is produced when a pasted image has been written to disk.
type ImageSavedMsg struct {
	RelativePath string
}

// ImageSaveErrorMsg is produced when image saving fails.
type ImageSaveErrorMsg struct {
	Err error
}
