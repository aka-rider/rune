package workspace

import (
	"os"
	"strings"
	"testing"

	"rune/pkg/ui/help"
)

// TestDocView_Accessors locks the discriminant ↔ accessor contract: kind drives
// IsFile/IsUntitled/IsHelp and Path(), so "untitled"/"help" are never decoded
// from a magic string (§1.7).
func TestDocView_Accessors(t *testing.T) {
	base := diskBaseline{size: 7, valid: true}
	f := fileView("/x/note.md", 42, base)
	if !f.IsFile() || f.IsUntitled() || f.IsHelp() {
		t.Errorf("fileView kind wrong: %+v", f)
	}
	if f.Path() != "/x/note.md" || f.DocID() != 42 || f.Baseline() != base {
		t.Errorf("fileView accessors wrong: %+v", f)
	}

	u := untitledView(9)
	if !u.IsUntitled() || u.IsFile() || u.Path() != "" || u.DocID() != 9 {
		t.Errorf("untitledView wrong: %+v", u)
	}
	// Zero value is a meaningful untitled (§1.1).
	if (docView{}).IsUntitled() != true {
		t.Error("zero docView must be untitled")
	}

	h := helpView()
	if !h.IsHelp() || h.Path() != help.DocPath || h.DocID() != 0 {
		t.Errorf("helpView wrong: %+v", h)
	}

	// withBaseline / withDocID copy one field, preserving kind + the rest.
	if got := f.withBaseline(diskBaseline{size: 99}); got.Path() != "/x/note.md" || got.DocID() != 42 || got.Baseline().size != 99 || !got.IsFile() {
		t.Errorf("withBaseline wrong: %+v", got)
	}
	if got := f.withDocID(100); got.Path() != "/x/note.md" || got.DocID() != 100 || got.Baseline() != base {
		t.Errorf("withDocID wrong: %+v", got)
	}
}

// TestDocView_NoRawLiteralsOutsideDocview is the structural guard for the
// consolidation: the displayed-document value must be built ONLY through the
// fileView/untitledView/helpView/withX constructors, never a raw docView{} struct
// literal — that is what keeps the kind discriminant authoritative and the editor
// buffer ↔ identity atomic. The constructors themselves live in docview.go.
func TestDocView_NoRawLiteralsOutsideDocview(t *testing.T) {
	entries, err := os.ReadDir(".")
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range entries {
		name := e.Name()
		if !strings.HasSuffix(name, ".go") || name == "docview.go" || strings.HasSuffix(name, "_test.go") {
			continue
		}
		b, err := os.ReadFile(name)
		if err != nil {
			t.Fatal(err)
		}
		for i, line := range strings.Split(string(b), "\n") {
			if strings.Contains(line, "docView{") {
				t.Errorf("%s:%d builds a raw docView{} literal; use a constructor (fileView/untitledView/helpView/withX): %s",
					name, i+1, strings.TrimSpace(line))
			}
		}
	}
}
