package editor

import (
	"os"
	"path/filepath"
	"testing"
)

func TestClassifyLink(t *testing.T) {
	tests := []struct {
		raw    string
		expect linkResolutionMode
	}{
		{"image.png", resolveFromFileDir},           // basename
		{"note", resolveFromFileDir},                // basename no ext
		{"./image.png", resolveFromFileDir},         // explicit relative
		{"./note", resolveFromFileDir},              // explicit relative no ext
		{"a/b/image.png", resolveFromCWD},           // path
		{"subdir/note", resolveFromCWD},             // path no ext
		{"/abs/path/file.md", resolveAbsolute},      // absolute
		{"http://example.com", resolveSkip},         // http
		{"https://example.com/img.png", resolveSkip}, // https
		{"data:image/png;base64,abc", resolveSkip},  // data URL
		{"", resolveSkip},                           // empty
	}
	for _, tc := range tests {
		t.Run(tc.raw, func(t *testing.T) {
			got := classifyLink(tc.raw)
			if got != tc.expect {
				t.Errorf("classifyLink(%q) = %v, want %v", tc.raw, got, tc.expect)
			}
		})
	}
}

func TestResolveLink(t *testing.T) {
	// Create a temp directory structure for testing
	tmpDir := t.TempDir()
	fileDir := filepath.Join(tmpDir, "notes")
	os.MkdirAll(fileDir, 0755)

	// Create test files
	notePath := filepath.Join(fileDir, "current.md")
	os.WriteFile(notePath, []byte("# current"), 0644)

	imagePath := filepath.Join(fileDir, "photo.png")
	os.WriteFile(imagePath, []byte("fake image"), 0644)

	// Create a sibling note
	siblingPath := filepath.Join(fileDir, "sibling.md")
	os.WriteFile(siblingPath, []byte("# sibling"), 0644)

	tests := []struct {
		name     string
		raw      string
		filePath string
		appendMD bool
		exist    bool
		want     string
	}{
		{
			name: "basename image from file dir",
			raw:  "photo.png", filePath: notePath, appendMD: false, exist: true,
			want: imagePath,
		},
		{
			name: "basename note with .md append",
			raw:  "sibling", filePath: notePath, appendMD: true, exist: false,
			want: siblingPath,
		},
		{
			name: "explicit ./ from file dir no fallback",
			raw:  "./photo.png", filePath: notePath, appendMD: false, exist: true,
			want: imagePath,
		},
		{
			name: "absolute path",
			raw:  imagePath, filePath: notePath, appendMD: false, exist: false,
			want: imagePath,
		},
		{
			name: "http url skipped",
			raw:  "http://example.com/img.png", filePath: notePath, appendMD: false, exist: false,
			want: "",
		},
		{
			name: "empty skipped",
			raw:  "", filePath: notePath, appendMD: false, exist: false,
			want: "",
		},
		{
			// When filePath is empty, there is no fileDir to resolve from.
			// With existCheck=true, the CWD fallback requires the file to
			// actually exist in CWD — which "photo.png" does not in the
			// test environment's CWD (t.TempDir() is isolated).
			// Result: empty string (no match found).
			name: "no file path basename with exist check returns empty",
			raw:  "photo.png", filePath: "", appendMD: false, exist: true,
			want: "",
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			got := resolveLink(tc.raw, tc.filePath, tc.appendMD, tc.exist)
			if got != tc.want {
				t.Errorf("resolveLink(%q, %q, %v, %v) = %q, want %q",
					tc.raw, tc.filePath, tc.appendMD, tc.exist, got, tc.want)
			}
		})
	}
}
