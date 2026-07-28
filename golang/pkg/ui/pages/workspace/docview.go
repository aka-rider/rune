package workspace

import (
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/help"
)

// docView is the single settled source of truth for which document the editor is
// displaying, and the cure for the former "active document" smear across
// filePath/docID/baseline. It is mutated only at a settled transition (the entry
// points in workspace_nav.go and the gen-matched FileLoadedMsg success), never at
// load start — so the editor buffer always corresponds to it.
//
// WP5: no more per-view size/mtime fingerprint — every divergence decision is
// driven by docstate.SyncState (Sync/Probe/Load), never a workspace-cached
// snapshot of disk metadata. docView keeps exactly kind/path/docID.
//
// Unexported fields + accessors make raw reads impossible outside this file, so
// "untitled" and "help" are carried by the discriminant docKind, never by a magic
// empty/sentinel string a reader must decode (§1.7). Branch on Kind()/IsX() for
// behaviour; Path() is for disk I/O and tab/breadcrumb identity only.
type docView struct {
	kind  docKind
	path  string // valid only when kind == docFile
	docID int64  // VFS store id; 0 before the store binds an untitled
}

// docKind discriminates what the editor is displaying. The zero value docUntitled
// is meaningful (§1.1): a fresh model with no file shows an untitled buffer, which
// is exactly the startup state.
type docKind uint8

const (
	docUntitled docKind = iota // VFS scratch / pre-store scratch; no disk path
	docFile                    // a bound .md on disk
	docHelp                    // the read-only built-in help document
)

func fileView(path string, docID int64) docView {
	return docView{kind: docFile, path: path, docID: docID}
}
func untitledView(docID int64) docView { return docView{kind: docUntitled, docID: docID} }
func helpView() docView                { return docView{kind: docHelp} }

func (v docView) Kind() docKind    { return v.kind }
func (v docView) IsFile() bool     { return v.kind == docFile }
func (v docView) IsUntitled() bool { return v.kind == docUntitled }
func (v docView) IsHelp() bool     { return v.kind == docHelp }
func (v docView) DocID() int64     { return v.docID }

// Path returns the disk path for a file, help.DocPath for help, or "" for an
// untitled doc.
func (v docView) Path() string {
	switch v.kind {
	case docFile:
		return v.path
	case docHelp:
		return help.DocPath
	default:
		return ""
	}
}

// Handle is the opentabs identity of the active document (used by finalize TAB-SET).
func (v docView) Handle() opentabs.TabHandle {
	return opentabs.TabHandle{DocID: v.docID, Path: v.Path()}
}

// withDocID returns a copy with only the docID replaced — StoreReadyMsg late
// binding of a file opened before the store was ready.
func (v docView) withDocID(docID int64) docView { v.docID = docID; return v }
