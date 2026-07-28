package workspace

import (
	"errors"
	"strings"
	"testing"

	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/markdownedit"
)

// TestImageSaveErrorSurfacesOnFooter is B2's regression test:
// markdownedit.ImageErrorMsg's own Update handler is a no-op (the only
// handler tree-wide before this fix), so an image-write failure
// (commands_image.go) or a ReplaceRange failure routed there surfaced
// nowhere. The workspace's Update must intercept the message and surface it
// via the existing errorCmd chokepoint.
func TestImageSaveErrorSurfacesOnFooter(t *testing.T) {
	m := newTestWorkspace(t)

	wantErr := errors.New("write image \"assets/pasted.png\": permission denied")
	_, cmd := m.Update(markdownedit.ImageErrorMsg{Err: wantErr})
	if cmd == nil {
		t.Fatal("expected a non-nil Cmd surfacing the image save error")
	}

	found := false
	for _, msg := range execCmds(cmd) {
		if e, ok := msg.(footer.ShowErrorMsg); ok {
			if strings.Contains(e.Text, wantErr.Error()) {
				found = true
			}
		}
	}
	if !found {
		t.Fatal("expected a footer.ShowErrorMsg surfacing the image save error")
	}
}
