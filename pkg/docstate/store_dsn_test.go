package docstate

import (
	"os"
	"path/filepath"
	"testing"
)

// TestSqliteURIPath_EscapesSignificantBytes pins sqliteURIPath's contract
// directly: only '%', '?', and '#' — the bytes SQLite's own "file:" URI
// parser treats as syntactically significant — are percent-encoded; every
// other byte (notably '/', the path separator) passes through untouched.
func TestSqliteURIPath_EscapesSignificantBytes(t *testing.T) {
	cases := []struct{ in, want string }{
		{"/plain/path/rune.db", "/plain/path/rune.db"},
		{"/vault?special/rune.db", "/vault%3Fspecial/rune.db"},
		{"/vault%25/rune.db", "/vault%2525/rune.db"},
		{"/vault#frag/rune.db", "/vault%23frag/rune.db"},
		{"/a?b%c#d", "/a%3Fb%25c%23d"},
	}
	for _, c := range cases {
		if got := sqliteURIPath(c.in); got != c.want {
			t.Errorf("sqliteURIPath(%q) = %q, want %q", c.in, got, c.want)
		}
	}
}

// TestOpenAt_WorkDirContainingQuestionMark is S3's end-to-end regression
// test: a workDir/DB path containing '?' (or '%') must open, persist, and
// reopen correctly — a naive fmt.Sprintf("file:%s", path) DSN would have
// SQLite's own URI parser split the path at the first unescaped '?',
// silently truncating it and misreading the remainder as bogus query
// parameters (potentially dropping our own _txlock/_busy_timeout suffix
// too, since '#' terminates the URI outright).
func TestOpenAt_WorkDirContainingQuestionMark(t *testing.T) {
	base := t.TempDir()
	dir := filepath.Join(base, "vault?with%special#chars")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatalf("mkdir special dir: %v", err)
	}

	s, warn, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("OpenAt(%q): %v", dir, err)
	}
	if warn != "" {
		t.Fatalf("OpenAt(%q): unexpected degradation warning %q (DSN likely misparsed, fell back to :memory:)", dir, warn)
	}
	docID := testDoc(t, s)
	if _, err := s.AppendEdit(docID, singleInsert("hello"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}
	if err := s.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}

	// The DB file must have landed at the LITERAL requested path — not a
	// truncated one — proving the DSN was never misparsed.
	if _, statErr := os.Stat(filepath.Join(dir, "rune.db")); statErr != nil {
		t.Fatalf("rune.db not found at the literal requested dir %q: %v", dir, statErr)
	}

	// Reopen at the SAME dir and verify the journaled edit survived —
	// end-to-end proof the write landed in the right file. OpenAt always
	// establishes a NEW session (v10), so s2 is a genuinely different
	// session from s — RecoverDocument alone would correctly see nothing
	// (s2 never touched docID itself); RecoverAcrossSessions is the
	// deliberate cross-session read, and s's session must be forced "dead"
	// here since it shares this SAME test process's pid with s2 (a real
	// second rune process's pid would already read as gone — this override
	// only compensates for both "sessions" running in one os process here).
	s2, warn2, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("reopen OpenAt(%q): %v", dir, err)
	}
	defer s2.Close()
	if warn2 != "" {
		t.Fatalf("reopen: unexpected degradation warning %q", warn2)
	}
	s2.SetLivenessCheck(func(pid int, startedAt string) bool { return false })
	content, found, err := s2.RecoverAcrossSessions(docID)
	if err != nil {
		t.Fatalf("RecoverAcrossSessions after reopen: %v", err)
	}
	if !found {
		t.Fatal("RecoverAcrossSessions after reopen: found=false, want the prior session's journaled content")
	}
	if content != "hello" {
		t.Fatalf("content after reopen = %q, want %q", content, "hello")
	}
}
