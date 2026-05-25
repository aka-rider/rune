package editortest

import (
	"flag"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

var update = flag.Bool("update", false, "update golden files")

// GoldenFile compares actual content against a golden file.
// If -update is set, creates or overwrites the golden file.
// Otherwise, compares and fails the test on mismatch.
func GoldenFile(t *testing.T, goldenPath, actual string) {
	t.Helper()

	if *update {
		// Ensure directory exists
		dir := filepath.Dir(goldenPath)
		if err := os.MkdirAll(dir, 0o755); err != nil {
			t.Fatalf("golden: failed to create directory %q: %v", dir, err)
		}
		if err := os.WriteFile(goldenPath, []byte(actual), 0o644); err != nil {
			t.Fatalf("golden: failed to write golden file %q: %v", goldenPath, err)
		}
		t.Logf("golden: wrote %s", goldenPath)
		return
	}

	// Read expected content
	expected, err := os.ReadFile(goldenPath)
	if err != nil {
		if os.IsNotExist(err) {
			t.Fatalf("golden: golden file %q does not exist (run with -update to create)", goldenPath)
		}
		t.Fatalf("golden: failed to read golden file %q: %v", goldenPath, err)
	}

	if actual != string(expected) {
		// Show diff on failure
		diffs := UnifiedDiff([]byte(actual), expected)
		t.Errorf("golden: mismatch in %s\n%s", goldenPath, diffs)
	}
}

// ReadGoldenFile reads the content of a golden file.
func ReadGoldenFile(t *testing.T, goldenPath string) string {
	t.Helper()

	content, err := os.ReadFile(goldenPath)
	if err != nil {
		if os.IsNotExist(err) {
			t.Fatalf("golden: golden file %q does not exist (run with -update to create)", goldenPath)
		}
		t.Fatalf("golden: failed to read golden file %q: %v", goldenPath, err)
	}
	return string(content)
}

// EnsureGoldenDir creates the directory for a golden file path if it doesn't exist.
func EnsureGoldenDir(t *testing.T, goldenPath string) {
	t.Helper()
	dir := filepath.Dir(goldenPath)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatalf("golden: failed to create directory %q: %v", dir, err)
	}
}

// HasGoldenFile reports whether a golden file exists.
func HasGoldenFile(goldenPath string) bool {
	_, err := os.Stat(goldenPath)
	return err == nil
}

// TestDir returns the test golden directory path.
func TestDir(t *testing.T) string {
	t.Helper()
	// Use the package name and test name to create a unique golden directory
	return filepath.Join("testdata", t.Name())
}

// GoldenPath returns a golden file path for the given test and suffix.
func GoldenPath(t *testing.T, suffix string) string {
	t.Helper()
	// Replace slashes in test name with underscores for filesystem safety
	name := strings.ReplaceAll(t.Name(), "/", "_")
	return filepath.Join("testdata", name+"."+suffix)
}
