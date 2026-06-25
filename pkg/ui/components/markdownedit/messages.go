package markdownedit

// LinkActivatedMsg is emitted when the user follows a link — by double-clicking
// it or pressing Enter/Ctrl+Enter with the caret on it. The editor has already
// resolved the target (one resolver, §link_resolution.go); the workspace just
// branches on Kind and never decodes an empty string (§1.7).
type LinkActivatedMsg struct {
	Raw  string   // the target as written — for the footer + "not found" message
	Kind LinkKind // the discriminant the workspace acts on
	Dest string   // external URL (LinkExternal) or existing abs path (LinkInternal); "" for LinkMissing
}

// LinkKind discriminates how a followed link is handled. The zero value is
// LinkMissing — inert (nothing to open, just reported), so a zero-valued message
// can never accidentally launch the OS opener or a file.
type LinkKind uint8

const (
	LinkMissing  LinkKind = iota // internal target does not exist → report it (safe zero value)
	LinkInternal                 // Dest is an existing file path → open in rune
	LinkExternal                 // Dest is a URL → OS default handler
)

// ImageSavedMsg is produced when a pasted image has been written to disk.
type ImageSavedMsg struct {
	RelativePath string
}

// ImageSaveErrorMsg is produced when image saving fails.
type ImageSaveErrorMsg struct {
	Err error
}
