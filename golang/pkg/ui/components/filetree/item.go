package filetree

// Entry is a single item in the flat file list.
type Entry struct {
	Name  string
	Path  string
	IsDir bool
}
