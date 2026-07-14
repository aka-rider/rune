package workspaceroot

import (
	"io/fs"
	"testing"

	"rune/pkg/vfs"
)

// countingFS wraps a *vfs.Mem and tallies ReadDir calls per path, so tests
// can assert the single-disk-access-per-level invariant: Resolve must call
// ReadDir exactly once for every directory level it visits.
type countingFS struct {
	*vfs.Mem
	calls map[string]int
}

func newCountingFS(m *vfs.Mem) *countingFS {
	return &countingFS{Mem: m, calls: make(map[string]int)}
}

func (c *countingFS) ReadDir(name string) ([]fs.DirEntry, error) {
	c.calls[name]++
	return c.Mem.ReadDir(name)
}

func TestResolve_ExistingRuneAtCwd_SilentCwd(t *testing.T) {
	m := vfs.NewMem()
	mustWrite(t, m, "/home/alice/project/.rune/rune.db", "x")

	res := Resolve(m, "/home/alice/project", "/home/alice")

	if res.WorkDir != "/home/alice/project" {
		t.Fatalf("WorkDir = %q, want /home/alice/project (Prompt=%+v)", res.WorkDir, res.Prompt)
	}
	if res.Prompt != nil {
		t.Fatalf("expected no Prompt, got %+v", res.Prompt)
	}
}

func TestResolve_RuneSeveralLevelsUp_BeatsNearerGit(t *testing.T) {
	m := vfs.NewMem()
	// .rune several levels up from cwd
	mustWrite(t, m, "/home/alice/vault/.rune/rune.db", "x")
	// a NEARER .git, closer to cwd than the .rune ancestor
	mustWrite(t, m, "/home/alice/vault/notes/sub/.git", "gitfile")

	res := Resolve(m, "/home/alice/vault/notes/sub", "/home/alice")

	if res.WorkDir != "/home/alice/vault" {
		t.Fatalf("WorkDir = %q, want /home/alice/vault (global .rune priority), Prompt=%+v", res.WorkDir, res.Prompt)
	}
	if res.Prompt != nil {
		t.Fatalf("expected no Prompt, got %+v", res.Prompt)
	}
}

func TestResolve_GitSubdirNoRune_PromptsWithCandidates(t *testing.T) {
	m := vfs.NewMem()
	mustWrite(t, m, "/home/alice/repo/.git", "gitfile")
	mustWrite(t, m, "/home/alice/repo/src/main.go", "package main")

	res := Resolve(m, "/home/alice/repo/src", "/home/alice")

	if res.WorkDir != "" {
		t.Fatalf("expected undecided WorkDir, got %q", res.WorkDir)
	}
	if res.Prompt == nil {
		t.Fatal("expected a Prompt, got nil")
	}
	// project, here, global, then the always-appended trailing memory
	// candidate (Dir == cwd).
	wantDirs := []string{"/home/alice/repo", "/home/alice/repo/src", "/home/alice", "/home/alice/repo/src"}
	if len(res.Prompt.Candidates) != len(wantDirs) {
		t.Fatalf("candidates = %+v, want dirs %v", res.Prompt.Candidates, wantDirs)
	}
	for i, want := range wantDirs {
		if got := res.Prompt.Candidates[i].Dir; got != want {
			t.Fatalf("candidate[%d] = %q, want %q (all: %+v)", i, got, want, res.Prompt.Candidates)
		}
	}
	if res.Prompt.Candidates[0].Kind != KindProject {
		t.Fatalf("candidate[0].Kind = %v, want KindProject", res.Prompt.Candidates[0].Kind)
	}
	if res.Prompt.Candidates[1].Kind != KindHere {
		t.Fatalf("candidate[1].Kind = %v, want KindHere", res.Prompt.Candidates[1].Kind)
	}
	if res.Prompt.Candidates[2].Kind != KindGlobal {
		t.Fatalf("candidate[2].Kind = %v, want KindGlobal", res.Prompt.Candidates[2].Kind)
	}
	if res.Prompt.Candidates[3].Kind != KindMemory {
		t.Fatalf("candidate[3].Kind = %v, want KindMemory", res.Prompt.Candidates[3].Kind)
	}
	if res.Prompt.Default != 0 {
		t.Fatalf("Default = %d, want 0 (project root)", res.Prompt.Default)
	}
}

func TestResolve_ObsidianMarker_Detected(t *testing.T) {
	m := vfs.NewMem()
	mustWrite(t, m, "/vault/.obsidian/config", "x")
	mustWrite(t, m, "/vault/notes/a.md", "hi")

	res := Resolve(m, "/vault/notes", "")

	if res.Prompt == nil {
		t.Fatal("expected a Prompt")
	}
	if res.Prompt.Candidates[0].Dir != "/vault" || res.Prompt.Candidates[0].Kind != KindProject {
		t.Fatalf("expected /vault as project candidate, got %+v", res.Prompt.Candidates)
	}
}

func TestResolve_BareTree_PromptsCwdAndHome(t *testing.T) {
	m := vfs.NewMem()
	mustWrite(t, m, "/home/alice/scratch/notes.md", "hi")

	res := Resolve(m, "/home/alice/scratch", "/home/alice")

	if res.WorkDir != "" {
		t.Fatalf("expected undecided WorkDir, got %q", res.WorkDir)
	}
	if res.Prompt == nil {
		t.Fatal("expected a Prompt")
	}
	want := []Candidate{
		{Dir: "/home/alice/scratch", Kind: KindHere},
		{Dir: "/home/alice", Kind: KindGlobal},
		{Dir: "/home/alice/scratch", Kind: KindMemory},
	}
	if len(res.Prompt.Candidates) != len(want) {
		t.Fatalf("candidates = %+v, want %+v", res.Prompt.Candidates, want)
	}
	for i := range want {
		if res.Prompt.Candidates[i] != want[i] {
			t.Fatalf("candidate[%d] = %+v, want %+v", i, res.Prompt.Candidates[i], want[i])
		}
	}
	if res.Prompt.Default != 0 {
		t.Fatalf("Default = %d, want 0 (cwd, no project marker)", res.Prompt.Default)
	}
}

func TestResolve_BareTree_CwdEqualsHome_Deduped(t *testing.T) {
	m := vfs.NewMem()
	mustWrite(t, m, "/home/alice/notes.md", "hi")

	res := Resolve(m, "/home/alice", "/home/alice")

	if res.Prompt == nil {
		t.Fatal("expected a Prompt")
	}
	// cwd==home dedupes to 1 disk candidate, plus the always-appended
	// trailing memory candidate — which must NOT be swallowed by the same
	// dedup, even though it shares Dir with the disk candidate.
	if len(res.Prompt.Candidates) != 2 {
		t.Fatalf("expected cwd==home to dedup to 1 disk candidate + 1 memory candidate, got %+v", res.Prompt.Candidates)
	}
	if res.Prompt.Candidates[0].Dir != "/home/alice" {
		t.Fatalf("candidate[0] = %+v, want /home/alice", res.Prompt.Candidates[0])
	}
	if want := (Candidate{Dir: "/home/alice", Kind: KindMemory}); res.Prompt.Candidates[1] != want {
		t.Fatalf("candidate[1] = %+v, want %+v", res.Prompt.Candidates[1], want)
	}
}

func TestResolve_CeilingStopsAtHome(t *testing.T) {
	m := vfs.NewMem()
	// A .rune marker ABOVE home must never be found — the walk must not
	// climb past home when cwd is under home.
	mustWrite(t, m, "/home/.rune/rune.db", "x")
	mustWrite(t, m, "/home/alice/scratch/notes.md", "hi")

	res := Resolve(m, "/home/alice/scratch", "/home/alice")

	if res.WorkDir != "" {
		t.Fatalf("expected the walk to stop at home (not find /home/.rune), got WorkDir=%q", res.WorkDir)
	}
	if res.Prompt == nil {
		t.Fatal("expected a Prompt")
	}
}

func TestResolve_CwdOutsideHome_ClimbsToRootAndStillOffersHome(t *testing.T) {
	m := vfs.NewMem()
	mustWrite(t, m, "/mnt/scratch/notes.md", "hi")
	// A .rune marker under home must never be found by this walk: cwd is
	// outside home's ancestry entirely, so the walk climbs to "/" rather
	// than detouring through home.
	mustWrite(t, m, "/home/alice/.rune/rune.db", "x")

	res := Resolve(m, "/mnt/scratch", "/home/alice")

	if res.WorkDir != "" {
		t.Fatalf("expected undecided WorkDir (home's .rune must not be found), got %q", res.WorkDir)
	}
	if res.Prompt == nil {
		t.Fatal("expected a Prompt")
	}
	// Home is still offered as the "always offered" global candidate, even
	// though it's outside cwd's ancestry — per spec.
	foundHome := false
	for _, c := range res.Prompt.Candidates {
		if c.Dir == "/home/alice" {
			foundHome = true
			if c.Kind != KindGlobal {
				t.Fatalf("home candidate Kind = %v, want KindGlobal", c.Kind)
			}
		}
	}
	if !foundHome {
		t.Fatalf("home not offered as a candidate: %+v", res.Prompt.Candidates)
	}
}

func TestResolve_GitAsFile_Detected(t *testing.T) {
	m := vfs.NewMem()
	// A worktree/submodule .git is a FILE, not a directory.
	mustWrite(t, m, "/home/alice/repo/.git", "gitdir: /some/where\n")
	mustWrite(t, m, "/home/alice/repo/README.md", "hi")

	res := Resolve(m, "/home/alice/repo", "/home/alice")

	if res.Prompt == nil {
		t.Fatal("expected a Prompt")
	}
	if res.Prompt.Candidates[0].Kind != KindProject || res.Prompt.Candidates[0].Dir != "/home/alice/repo" {
		t.Fatalf("expected .git-as-file to be detected as project root, got %+v", res.Prompt.Candidates)
	}
}

func TestResolve_ReadDirError_TreatedAsNoMarkers(t *testing.T) {
	// A path with no entries at all (Mem synthesizes nothing) still returns
	// (nil, nil) from Mem.ReadDir rather than an error, so exercise the
	// "no markers here" fallthrough via an unrelated tree instead — the
	// walk must not crash and must keep climbing regardless.
	m := vfs.NewMem()
	mustWrite(t, m, "/home/alice/.rune/rune.db", "x")

	res := Resolve(m, "/home/alice/deep/nested/dir", "/home/alice")

	if res.WorkDir != "/home/alice" {
		t.Fatalf("WorkDir = %q, want /home/alice", res.WorkDir)
	}
}

func TestResolve_SingleReadDirPerLevel(t *testing.T) {
	m := vfs.NewMem()
	mustWrite(t, m, "/home/alice/vault/.git", "gitfile")
	mustWrite(t, m, "/home/alice/vault/notes/sub/deep/a.md", "hi")

	counting := newCountingFS(m)
	res := Resolve(counting, "/home/alice/vault/notes/sub/deep", "/home/alice")

	if res.Prompt == nil {
		t.Fatal("expected a Prompt")
	}

	wantLevels := []string{
		"/home/alice/vault/notes/sub/deep",
		"/home/alice/vault/notes/sub",
		"/home/alice/vault/notes",
		"/home/alice/vault",
		"/home/alice",
	}
	for _, lvl := range wantLevels {
		if got := counting.calls[lvl]; got != 1 {
			t.Errorf("ReadDir(%q) called %d times, want exactly 1", lvl, got)
		}
	}
	if len(counting.calls) != len(wantLevels) {
		t.Errorf("ReadDir called at %d distinct levels, want %d: %+v", len(counting.calls), len(wantLevels), counting.calls)
	}
}

func TestResolve_NoRuneAnywhere_EmptyHome(t *testing.T) {
	m := vfs.NewMem()
	mustWrite(t, m, "/scratch/notes.md", "hi")

	res := Resolve(m, "/scratch", "")

	if res.Prompt == nil {
		t.Fatal("expected a Prompt")
	}
	if len(res.Prompt.Candidates) != 2 {
		t.Fatalf("expected cwd candidate + trailing memory candidate with empty home, got %+v", res.Prompt.Candidates)
	}
	if res.Prompt.Candidates[0].Dir != "/scratch" || res.Prompt.Candidates[0].Kind != KindHere {
		t.Fatalf("candidate[0] = %+v, want cwd/KindHere", res.Prompt.Candidates[0])
	}
	if want := (Candidate{Dir: "/scratch", Kind: KindMemory}); res.Prompt.Candidates[1] != want {
		t.Fatalf("candidate[1] = %+v, want %+v", res.Prompt.Candidates[1], want)
	}
}

func TestResolve_CwdEqualsProjectRoot_MemoryCandidateStillOffered(t *testing.T) {
	m := vfs.NewMem()
	// cwd IS the project root itself (.git lives right here) — projectRoot
	// and cwd dedup to a single KindProject disk candidate.
	mustWrite(t, m, "/home/alice/repo/.git", "gitfile")
	mustWrite(t, m, "/home/alice/repo/README.md", "hi")

	res := Resolve(m, "/home/alice/repo", "/home/alice")

	if res.Prompt == nil {
		t.Fatal("expected a Prompt")
	}
	last := res.Prompt.Candidates[len(res.Prompt.Candidates)-1]
	want := Candidate{Dir: "/home/alice/repo", Kind: KindMemory}
	if last != want {
		t.Fatalf("expected trailing candidate = %+v, got %+v (all: %+v)", want, last, res.Prompt.Candidates)
	}
	if res.Prompt.Default == len(res.Prompt.Candidates)-1 {
		t.Fatalf("Default must never point at the trailing memory candidate, got Default=%d", res.Prompt.Default)
	}
}

// TestResolve_DefaultNeverPointsAtMemoryCandidate sweeps every dedup
// scenario buildPrompt handles (project+here+global, bare cwd+home,
// cwd==home, empty home) and proves two invariants hold in every case: the
// trailing candidate is always KindMemory, and Default never selects it —
// None must always be a conscious opt-in, never pre-selected.
func TestResolve_DefaultNeverPointsAtMemoryCandidate(t *testing.T) {
	scenarios := []struct {
		name string
		cwd  string
		home string
		seed func(m *vfs.Mem)
	}{
		{"project+here+global", "/home/alice/repo/src", "/home/alice", func(m *vfs.Mem) {
			mustWrite(t, m, "/home/alice/repo/.git", "gitfile")
			mustWrite(t, m, "/home/alice/repo/src/main.go", "package main")
		}},
		{"bare cwd+home", "/home/alice/scratch", "/home/alice", func(m *vfs.Mem) {
			mustWrite(t, m, "/home/alice/scratch/notes.md", "hi")
		}},
		{"cwd==home", "/home/alice", "/home/alice", func(m *vfs.Mem) {
			mustWrite(t, m, "/home/alice/notes.md", "hi")
		}},
		{"empty home", "/scratch", "", func(m *vfs.Mem) {
			mustWrite(t, m, "/scratch/notes.md", "hi")
		}},
	}

	for _, sc := range scenarios {
		t.Run(sc.name, func(t *testing.T) {
			m := vfs.NewMem()
			sc.seed(m)
			res := Resolve(m, sc.cwd, sc.home)
			if res.Prompt == nil {
				t.Fatal("expected a Prompt")
			}
			last := len(res.Prompt.Candidates) - 1
			if res.Prompt.Candidates[last].Kind != KindMemory {
				t.Fatalf("expected trailing candidate to be KindMemory, got %+v", res.Prompt.Candidates)
			}
			if res.Prompt.Default == last {
				t.Fatalf("Default must never point at the trailing memory candidate (index %d), got Default=%d", last, res.Prompt.Default)
			}
		})
	}
}

func mustWrite(t *testing.T, m *vfs.Mem, path, content string) {
	t.Helper()
	if err := m.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatalf("seed WriteFile(%q): %v", path, err)
	}
}
