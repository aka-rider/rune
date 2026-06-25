package workspace

import (
	"runtime"
	"testing"
)

// TestExternalOpener asserts the URL is passed as a separate exec argument (the
// injection-safety boundary — never a shell string) and that the platform
// command is chosen by GOOS.
func TestExternalOpener(t *testing.T) {
	name, args := externalOpener("https://example.com")
	if len(args) == 0 || args[len(args)-1] != "https://example.com" {
		t.Errorf("URL must be a separate arg; got name=%q args=%v", name, args)
	}
	want := map[string]string{"darwin": "open", "windows": "rundll32"}
	expect, ok := want[runtime.GOOS]
	if !ok {
		expect = "xdg-open"
	}
	if name != expect {
		t.Errorf("opener for %s = %q, want %q", runtime.GOOS, name, expect)
	}
}
