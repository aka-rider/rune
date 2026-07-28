package workspace

// §1.4.5 byte-faithful round-trip through the REAL key path: a file with a
// BOM, CRLF line endings, and no trailing newline is loaded, edited by one
// keystroke, and saved with ⌘S — the bytes on disk afterwards are identical
// to the buffer, and identical to the original except for the single
// inserted byte: no BOM strip, no CRLF normalization, no appended newline.
// The store-level companion is TestMaterialize_OverwriteWritesVerbatim
// (workspace_fileio_test.go); this covers the full key→materialize path.

import (
	"bytes"
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/internal/editortest"
	"rune/pkg/vfs"
)

func TestSaveKey_VerbatimBytes(t *testing.T) {
	const original = "\ufeff# T\r\nCRLF line\r\nlast line no eol"

	dir := t.TempDir()
	path := filepath.Join(dir, "hostile.md")
	if err := os.WriteFile(path, []byte(original), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newScrollWorkspace(t)).WithFS(vfs.Disk{})
	m = loadFile(m, path, original)
	m = focusEditor(m)

	// LOAD-VERBATIM (§1.4.5): the buffer holds the disk bytes exactly.
	if got := m.editor.Content(); got != original {
		t.Fatalf("load normalized the bytes:\n%s",
			editortest.UnifiedDiff([]byte(original), []byte(got)))
	}

	// Move into the middle of the first line (2 caret stops keeps the cursor
	// inside "# T" whether or not the BOM is its own stop), insert one byte.
	for range 2 {
		m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyRight})
	}
	m = typeChar(m, 'Z')

	edited := m.editor.Content()
	if len(edited) != len(original)+1 || strings.Count(edited, "Z") != 1 {
		t.Fatalf("expected exactly one inserted byte, got %q", edited)
	}

	// ⌘S through the real binding; settle the async materialize round trip.
	m, cmd := m.Update(tea.KeyPressMsg{Code: 's', Mod: tea.ModSuper})
	m = settle(t, m, cmd)

	disk, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	// SAVE-VERBATIM (§1.4.5): disk equals the buffer, byte for byte.
	if !bytes.Equal(disk, []byte(edited)) {
		t.Fatalf("disk bytes differ from the buffer:\n%s",
			editortest.UnifiedDiff([]byte(edited), disk))
	}
	// The hostile features survived: BOM intact, CRLF count unchanged, and
	// still no trailing newline.
	if !bytes.HasPrefix(disk, []byte("\ufeff")) {
		t.Fatal("BOM was stripped on save")
	}
	if got, want := bytes.Count(disk, []byte("\r\n")), strings.Count(original, "\r\n"); got != want {
		t.Fatalf("CRLF count changed: %d, want %d", got, want)
	}
	if disk[len(disk)-1] == '\n' {
		t.Fatal("a trailing newline was appended")
	}
}
